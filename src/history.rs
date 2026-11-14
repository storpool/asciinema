//! Best-effort shell history from terminal output and resizes alone.
//!
//! Newlines create candidates, not immediate submissions. Editor-mode changes
//! and subsequent prompts distinguish submission from completion and redraw.
//! Terminal cells supply the edited text; anchors follow its rows through reflow.

mod prompt;

use anyhow::Result;
use avt::ansi::{DecMode, Function};
use avt::{Anchor, Observation, Observer, Operation, Position, Row, ScreenKind, Terminal};

use crate::asciicast::{Asciicast, EventData};

pub fn extract(cast: Asciicast<'_>) -> Result<Vec<String>> {
    let mut terminal = Terminal::with_scrollback(
        cast.header.term_cols.into(),
        cast.header.term_rows.into(),
        Some(100),
    )?;

    let mut tracker = Tracker::default();

    for event in cast.events {
        match event?.data {
            EventData::Output(text) => {
                terminal.feed_with(&text, &mut tracker);
                tracker.end_output_event(&terminal);
            }

            EventData::Resize(cols, rows) => {
                terminal.resize_with(cols.into(), rows.into(), &mut tracker)
            }

            _ => (),
        }
    }

    tracker.finish_pending();

    Ok(tracker.commands)
}

// Observations have three boundaries: terminal operations, editor-mode changes,
// and recording events. LF captures a candidate; editor close confirms it; a
// later prompt or end of recording finalizes it. Continuations and redraws can
// reopen or replace the candidate before finalization.
#[derive(Default)]
struct Tracker {
    commands: Vec<String>,
    pending: Option<Pending>,
    paste_enabled: bool,
    editor_seen: bool,
    ready_prompt: Option<String>,
    // The editor disabled bracketed paste, possibly before its final redraw/LF.
    editor_closing: bool,
    // A transient prompt may replace several rows before the accepting LF.
    closing_redraw: bool,
    // Zsh cancellation closes paste mode but retains application cursor keys.
    cancelled: bool,
    // Updated only at output-event boundaries, not after each terminal operation.
    // Separately observed input distinguishes an edited exit from generated EOF.
    previous_event_had_input: bool,
    before_cr: Option<Anchor>,
    right_prompt: Option<RightPrompt>,
    printing_before_right: bool,
    bare_root: bool,
}

struct IndentedLine {
    anchor: Anchor,
    columns: usize,
    text: String,
}

struct RightPrompt {
    // Anchor the prompt row, since clearing suggestions erases its right cells.
    line: Anchor,
    columns: usize,
}

impl RightPrompt {
    fn position(&self, terminal: &Terminal) -> Option<Position> {
        let mut position = terminal.resolve(&self.line)?;
        let offset = position.col + self.columns;
        position.row += (offset / terminal.size().0) as isize;
        position.col = offset % terminal.size().0;

        Some(position)
    }
}

struct Pending {
    parts: Vec<String>,
    indented: Vec<IndentedLine>,
    prompt: String,
    anchor: Anchor,
    // Candidates from a bracketed-paste editor need its closing transition.
    requires_editor_close: bool,
    // Editor-close evidence is independent of the accepting newline: either
    // can arrive first. Plain-shell continuations can also confirm a candidate.
    confirmed: bool,
    // LF or downward acceptance was observed. A continuation reopens the line.
    line_complete: bool,
    possible_eof: bool,
    unedited: bool,
    output_seen: bool,
}

impl Pending {
    fn into_command(self) -> Option<String> {
        if !self.line_complete
            || (self.requires_editor_close && !self.confirmed)
            || self.possible_eof
        {
            return None;
        }

        let mut text = self.parts.join("\n");
        let tail = command_tail(&text);

        // Quote inference only rejects ambiguous one-line input. The scanner is
        // not a shell grammar and cannot overrule editor-confirmed submissions.
        // Multiline boundaries also allow literal quoting in here-doc bodies.
        if !self.confirmed && self.parts.len() == 1 && tail.needs_continuation {
            return None;
        }

        text.truncate(tail.trimmed.len());

        Some(text)
    }

    fn indented_line(&self, terminal: &Terminal, line: &Snapshot) -> Option<usize> {
        self.indented
            .iter()
            .position(|indent| terminal.resolve(&indent.anchor) == Some(line.start))
    }

    fn observe_output(&mut self, text: &str, same_line: bool, at_newline: bool) {
        // Explicit Bash exit echoes the command and then prints an exit message.
        // EOF at an empty prompt prints just the latter.
        if text.trim() == self.parts[0] {
            self.possible_eof = false;
        }

        // Remote shells localize their inline EOF message. An untouched prompt
        // followed immediately by SSH closure supplies language-independent evidence.
        if !self.requires_editor_close
            && self.unedited
            && !self.output_seen
            && self.parts.len() == 1
            && self.parts[0].split_whitespace().count() == 1
            && text.starts_with("Connection to ")
            && text.trim_end().ends_with(" closed.")
        {
            self.possible_eof = true;
        }

        if at_newline && !same_line && !text.trim().is_empty() {
            self.output_seen = true;
        }
    }
}

impl Tracker {
    fn end_output_event(&mut self, terminal: &Terminal) {
        self.previous_event_had_input = snapshot(terminal, None, self.right_prompt.as_ref())
            .is_some_and(|line| {
                self.input_boundary(&line.text)
                    .is_some_and(|end| !line.text[end..].trim_end().is_empty())
            });
    }

    // Prefer the prompt prefix observed before editing, otherwise use its shape.
    fn input_boundary(&self, text: &str) -> Option<usize> {
        self.ready_prompt
            .as_ref()
            .filter(|prompt| text.starts_with(prompt.as_str()))
            .map(|prompt| prompt.len())
            .or_else(|| prompt::primary_end(text))
    }

    // A recognizable shape alone is not enough in every application/editor context.
    fn primary_input_start(&self, text: &str) -> Option<usize> {
        self.input_boundary(text).filter(|end| {
            let editor_active = self.paste_enabled || self.editor_closing;
            let root_allowed = *end != 2 || !text.starts_with("# ") || self.bare_root;
            // This arrow also appears in narrated demo output.
            let arrow_allowed = !text.starts_with("➜ ") || editor_active;

            let shell_allowed =
                !self.editor_seen || editor_active || prompt::has_user_host_identity(&text[..*end]);

            root_allowed && arrow_allowed && shell_allowed
        })
    }

    fn discard_pending(&mut self) {
        self.pending = None;
    }

    fn paste_mode(&mut self, context: &mut Observation<'_>, enabled: bool) {
        let terminal = context.terminal();
        self.editor_seen |= enabled;

        if terminal.active_screen_kind() == ScreenKind::Primary {
            self.editor_closing = !enabled;
            self.cancelled = !enabled && terminal.cursor_key_app_mode();

            if enabled {
                // The arrow precedes the directory in this prompt family.
                // Learn its complete visible prefix before editing starts.
                self.ready_prompt = snapshot(terminal, None, self.right_prompt.as_ref())
                    .filter(|line| line.text.starts_with("➜  ") && line.text.ends_with(' '))
                    .map(|line| line.text);

                self.closing_redraw = false;
            } else {
                let line = snapshot(
                    terminal,
                    self.before_cr.as_ref(),
                    self.right_prompt.as_ref(),
                );

                self.closing_redraw = line.as_ref().is_some_and(|line| {
                    self.input_boundary(&line.text)
                        .is_none_or(|end| !line.text[end..].trim().is_empty())
                });

                if self.cancelled {
                    // An application's editor can close this way too. Its already
                    // accepted launcher remains a shell command.
                    if self
                        .pending
                        .as_ref()
                        .is_some_and(|p| p.line_complete && p.confirmed)
                    {
                        self.finish_pending();
                    }

                    self.discard_pending();
                } else {
                    if let Some(line) = line {
                        let anchor = context.anchor_at(line.start).unwrap();
                        self.capture(context.terminal(), line, anchor, false);
                    }

                    if let Some(pending) = &mut self.pending {
                        if pending.requires_editor_close {
                            pending.confirmed = true;
                        }
                    }
                }
            }
        }

        // Closing capture still uses the previous editor mode.
        self.paste_enabled = enabled;

        // Zsh paints continuation prompts before reopening the editor. Recheck
        // with paste mode active so a prior line's acceptance is not final.
        if enabled {
            self.see_prompt(context.terminal());
        }
    }

    fn same_line(&self, terminal: &Terminal, line: &Snapshot) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|pending| terminal.resolve(&pending.anchor) == Some(line.start))
    }

    fn capture(&mut self, terminal: &Terminal, line: Snapshot, anchor: Anchor, at_newline: bool) {
        if self.cancelled {
            return;
        }

        let same_line = self.same_line(terminal, &line);

        if let Some(end) = self.primary_input_start(&line.text) {
            self.start_candidate(&line.text, end, anchor, same_line, at_newline);

            return;
        }

        let Some(pending) = &mut self.pending else {
            return;
        };

        let continuation = prompt::continuation_end(&line.text).filter(|_| {
            ((!pending.requires_editor_close && !pending.line_complete) || self.paste_enabled)
                || (self.editor_closing && same_line)
        });

        if let Some(end) = continuation {
            self.extend_candidate(
                line.text[end..].to_owned(),
                anchor,
                same_line,
                at_newline,
                self.editor_closing || !self.paste_enabled,
            );

            return;
        }

        if let Some(index) = pending.indented_line(terminal, &line) {
            self.capture_indented(&line.text, index, at_newline);

            return;
        }

        // Pasted multiline buffers need not have continuation prompts.
        // Only extend an unconfirmed editor candidate, never its output.
        let editing = pending.requires_editor_close
            && (self.paste_enabled || self.editor_closing)
            && (!pending.confirmed || !pending.line_complete || (self.editor_closing && same_line))
            && (!line.text.is_empty() || (at_newline && self.paste_enabled));

        if editing {
            self.extend_candidate(
                line.text,
                anchor,
                same_line,
                at_newline,
                self.editor_closing,
            );
        } else {
            pending.observe_output(&line.text, same_line, at_newline);
        }
    }

    fn start_candidate(
        &mut self,
        text: &str,
        input_start: usize,
        anchor: Anchor,
        same_line: bool,
        at_newline: bool,
    ) {
        let command = &text[input_start..];

        if command.trim_end().ends_with("^C") {
            self.discard_pending();

            return;
        }

        if command.trim().is_empty() {
            return;
        }

        // A REPL can move back over its launcher's already accepted row during
        // setup. Do not turn that row into a new edit.
        if same_line
            && self.pending.as_ref().is_some_and(|pending| {
                pending.line_complete
                    && pending.confirmed
                    && pending.parts.len() == 1
                    && pending.parts[0] == command
            })
        {
            return;
        }

        // A primary-line redraw replaces the candidate's text, but its editor
        // rows still belong to the same buffer until finalization or disposal.
        let indented = self
            .pending
            .take()
            .map_or_else(Vec::new, |pending| pending.indented);

        self.pending = Some(Pending {
            parts: vec![command.to_owned()],
            indented,
            prompt: text[..input_start].to_owned(),
            anchor,
            requires_editor_close: self.paste_enabled || self.editor_closing,
            confirmed: self.editor_closing,
            line_complete: at_newline,
            possible_eof: matches!(command.trim_end(), "exit" | "logout")
                && !self.previous_event_had_input,
            unedited: !self.previous_event_had_input,
            output_seen: false,
        });

        if at_newline {
            self.bare_root = false;
        }
    }

    fn extend_candidate(
        &mut self,
        text: String,
        anchor: Anchor,
        same_line: bool,
        at_newline: bool,
        confirmed: bool,
    ) {
        if text.trim_end().ends_with("^C") {
            self.discard_pending();

            return;
        }

        let pending = self.pending.as_mut().unwrap();

        if same_line {
            *pending.parts.last_mut().unwrap() = text;
        } else {
            pending.parts.push(text);
        }

        pending.anchor = anchor;
        pending.line_complete = at_newline;
        pending.confirmed = confirmed;
    }

    fn capture_indented(&mut self, text: &str, index: usize, at_newline: bool) {
        let pending = self.pending.as_mut().unwrap();
        let prefix = text.len() - text.trim_start_matches(' ').len();
        let start = pending.indented[index].columns.min(prefix);
        let text = &text[start..];

        if text.trim_end().ends_with("^C") {
            self.discard_pending();

            return;
        }

        pending.indented[index].text = text.to_owned();
        pending.parts.truncate(1);

        pending.parts.extend(
            pending.indented[..=index]
                .iter()
                .map(|line| line.text.clone()),
        );

        pending.line_complete = at_newline;
        pending.confirmed = self.editor_closing;
    }

    fn see_prompt(&mut self, terminal: &Terminal) {
        let Some(line) = snapshot(terminal, None, self.right_prompt.as_ref()) else {
            return;
        };

        if line.text == "# " {
            self.bare_root = true;
        }

        if self.same_line(terminal, &line) {
            return;
        }

        let primary = self.input_boundary(&line.text);

        if primary.is_some() && self.pending.is_none() && !self.closing_redraw {
            self.editor_closing = false;
        }

        let Some(pending) = &mut self.pending else {
            return;
        };

        if let Some(end) = primary {
            if !pending.line_complete {
                return;
            }

            let command = line.text[end..].trim_end();

            if !command.is_empty()
                && (!pending.confirmed
                    && (pending.requires_editor_close
                        || (line.text[..end] == pending.prompt
                            && command.starts_with(&pending.parts[0]))))
            {
                // Completion and paste redisplays retain the editable buffer.
                self.bare_root = pending.prompt == "# ";
                self.discard_pending();
            } else {
                self.finish_pending();
                self.editor_closing = self.closing_redraw;
            }
        } else if let Some(end) = prompt::continuation_end(&line.text) {
            if !pending.output_seen
                && (!pending.requires_editor_close || self.paste_enabled)
                && (line.text[end..].trim().is_empty()
                    || (pending.parts.len() == 1
                        && command_tail(&pending.parts[0]).needs_continuation))
            {
                pending.line_complete = false;
            }
        }
    }

    fn finish_pending(&mut self) {
        if let Some(command) = self.pending.take().and_then(Pending::into_command) {
            self.commands.push(command);
        }
    }

    // Fish aligns multiline editing beneath the first input column and adds
    // syntax indentation. Remember cursor movement through the blank prefix,
    // including on redraw, so user-supplied indentation remains command text.
    fn see_indentation(&mut self, context: &mut Observation<'_>) {
        if !self.paste_enabled
            || !self.pending.as_ref().is_some_and(|pending| {
                pending.requires_editor_close && !(pending.line_complete && pending.confirmed)
            })
        {
            return;
        }

        let terminal = context.terminal();

        let Some(line) = snapshot(terminal, None, self.right_prompt.as_ref()) else {
            return;
        };

        if terminal.cursor().col == 0
            || line
                .text
                .chars()
                .take(terminal.cursor().col)
                .any(|ch| ch != ' ')
            || self.same_line(terminal, &line)
        {
            return;
        }

        let pending = self.pending.as_mut().unwrap();

        pending
            .indented
            .retain(|line| terminal.resolve(&line.anchor).is_some());

        if pending.indented_line(terminal, &line).is_none() {
            let columns = terminal.cursor().col;

            pending.indented.push(IndentedLine {
                anchor: context.anchor_at(line.start).unwrap(),
                columns,
                text: String::new(),
            });
        }
    }
}

impl Observer for Tracker {
    type Output = ();

    fn before(&mut self, context: &mut Observation<'_>, operation: &Operation<'_>) {
        match operation {
            Operation::Function(Function::Print(_)) => {
                let terminal = context.terminal();

                self.printing_before_right = self
                    .right_prompt
                    .as_ref()
                    .and_then(|p| p.position(terminal))
                    .is_some_and(|right| {
                        let cursor = terminal.cursor();

                        right.screen == terminal.active_screen_kind()
                            && right.row == cursor.row as isize
                            && cursor.col < right.col
                    });
            }

            Operation::Function(Function::Cr) if context.terminal().cursor().col > 0 => {
                self.before_cr = Some(context.anchor_at_cursor());
            }

            Operation::Function(Function::Lf) => {
                let terminal = context.terminal();

                if let Some(line) = snapshot(
                    terminal,
                    self.before_cr.as_ref(),
                    self.right_prompt.as_ref(),
                ) {
                    let anchor = context.anchor_at(line.start).unwrap();
                    self.capture(context.terminal(), line, anchor, true);
                }

                self.before_cr = None;
                self.closing_redraw = false;
            }

            _ => (),
        }
    }

    fn after(&mut self, context: &mut Observation<'_>, operation: &Operation<'_>) {
        match operation {
            Operation::Function(Function::Decset(modes) | Function::Decrst(modes))
                if modes.as_slice().contains(&DecMode::BracketedPaste) =>
            {
                let enabled = context.terminal().bracketed_paste_mode();
                self.paste_mode(context, enabled);
            }

            Operation::Function(Function::Ris) => {
                self.finish_pending();

                *self = Self {
                    commands: std::mem::take(&mut self.commands),
                    ..Self::default()
                };
            }

            Operation::Function(Function::Print(_)) => {
                let terminal = context.terminal();

                if self.printing_before_right
                    && self
                        .right_prompt
                        .as_ref()
                        .and_then(|p| p.position(terminal))
                        .is_some_and(|right| {
                            let cursor = terminal.cursor();

                            cursor.row as isize > right.row
                                || (cursor.row as isize == right.row && cursor.col > right.col)
                        })
                {
                    self.right_prompt = None;
                }

                self.see_prompt(context.terminal());
            }

            Operation::Function(Function::Cuf(_) | Function::Cha(_) | Function::Cup(_, _)) => {
                let terminal = context.terminal();

                // Jumping beyond the occupied cells paints a right prompt, including
                // when it reappears after a long edit is replaced by a shorter one.
                if self.paste_enabled {
                    if let Some(line) = snapshot(terminal, None, None) {
                        let cursor = terminal.cursor();
                        let row = terminal.screen().row(cursor.row).unwrap();

                        if line.start.row == cursor.row as isize
                            && self.input_boundary(&line.text).is_some()
                            && cursor.col > occupied_columns(row) + 1
                        {
                            let columns = cursor.col;

                            self.right_prompt = Some(RightPrompt {
                                line: context.anchor_at(line.start).unwrap(),
                                columns,
                            });
                        }
                    }
                }

                self.see_indentation(context);
            }

            Operation::Function(Function::Cud(_)) if self.editor_closing => {
                let terminal = context.terminal();

                let empty = snapshot(terminal, None, self.right_prompt.as_ref())
                    .is_some_and(|line| line.text.trim().is_empty());

                if let Some(pending) = &mut self.pending {
                    if empty
                        && pending.confirmed
                        && terminal
                            .resolve(&pending.anchor)
                            .is_some_and(|pos| pos.row != terminal.cursor().row as isize)
                    {
                        // Zsh can accept reverse search by moving down, without LF.
                        pending.line_complete = true;
                    }
                }
            }

            _ => (),
        }
    }

    fn end(&mut self, _: &mut Observation<'_>) {}
}

struct Snapshot {
    text: String,
    start: Position,
}

// Terminal columns, including the second cell of a wide character. String
// character counts cannot be compared with cursor coordinates.
fn occupied_columns(row: &Row) -> usize {
    row.cells()
        .iter()
        .enumerate()
        .rfind(|(_, cell)| !cell.char().is_whitespace())
        .map_or(0, |(column, cell)| column + usize::from(cell.width()))
}

fn snapshot(
    terminal: &Terminal,
    before_cr: Option<&Anchor>,
    right_prompt: Option<&RightPrompt>,
) -> Option<Snapshot> {
    if terminal.active_screen_kind() != ScreenKind::Primary {
        return None;
    }

    let screen = terminal.screen();
    let cursor = terminal.cursor();
    let line = screen.line_at(cursor.row as isize)?;
    let bounds = line.row_range();
    let mut last_row = bounds.end - 1;
    let extent = before_cr.and_then(|anchor| terminal.resolve(anchor));

    let right = right_prompt
        .and_then(|prompt| prompt.position(terminal))
        .filter(|right| {
            // A final cursor beyond the right prompt proves that input reached
            // into it, even when the last printed fragment started at its edge.
            !(cursor.row as isize == right.row && cursor.col > right.col)
                && !extent.is_some_and(|pos| pos.row == right.row && pos.col > right.col)
        });

    if let Some(right) = right {
        if right.screen == ScreenKind::Primary && bounds.contains(&right.row) {
            // A right prompt can reach the margin and mark its row as wrapped.
            last_row = right.row;
        }
    }

    let mut rows = line.rows();

    let mut text: String = rows
        .by_ref()
        .take((last_row - bounds.start) as usize)
        .flat_map(Row::chars)
        .collect();

    let last = rows.next()?;

    let mut cols = last
        .cells()
        .iter()
        .rposition(|cell| cell.char() != ' ')
        .map_or(0, |i| i + 1)
        .max(if cursor.row as isize == last_row {
            cursor.col
        } else {
            0
        })
        .max(
            extent
                .filter(|pos| pos.screen == ScreenKind::Primary && pos.row == last_row)
                .map_or(0, |pos| pos.col),
        );

    if let Some(right) = right {
        if right.screen == ScreenKind::Primary && right.row == last_row {
            cols = cols.min(right.col);
        }
    }

    text.extend(
        last.cells()[..cols]
            .iter()
            .filter(|cell| cell.width() > 0)
            .map(|cell| cell.char()),
    );

    Some(Snapshot {
        text,
        start: Position {
            screen: ScreenKind::Primary,
            row: bounds.start,
            col: 0,
        },
    })
}

struct CommandTail<'a> {
    trimmed: &'a str,
    needs_continuation: bool,
}

// Only discard final shell separators. Spaces in quotes or escaped spaces are
// arguments; spaces before internal newlines can belong to quotes or here-docs.
// The continuation hint is deliberately approximate, not a submission verdict.
fn command_tail(text: &str) -> CommandTail<'_> {
    let mut quote = None;
    let mut escaped = false;
    let mut comment = false;
    let mut word_boundary = true;
    let mut end = 0;

    for (i, ch) in text.char_indices() {
        if escaped || quote.is_some() || !matches!(ch, ' ' | '\t' | '\n') {
            end = i + ch.len_utf8();
        }

        if comment {
            comment = ch != '\n';
        } else if escaped {
            escaped = false;
        } else if ch == '\\' && quote != Some('\'') {
            escaped = true;
        } else if quote == Some(ch) {
            quote = None;
        } else if quote.is_none() && matches!(ch, '\'' | '"' | '`') {
            quote = Some(ch);
        } else if quote.is_none() && ch == '#' && word_boundary {
            comment = true;
        }

        word_boundary = ch.is_whitespace() || matches!(ch, ';' | '|' | '&' | '(' | ')');
    }

    CommandTail {
        trimmed: &text[..end],
        needs_continuation: escaped || quote.is_some(),
    }
}

#[cfg(test)]
mod tests;
