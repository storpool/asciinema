//! Prompt shapes only. The tracker decides whether the surrounding output is input.

pub(super) fn continuation_end(text: &str) -> Option<usize> {
    [
        "> ",
        "quote> ",
        "dquote> ",
        "heredoc> ",
        "for> ",
        "while> ",
        "if> ",
    ]
    .iter()
    .find(|prefix| text.starts_with(**prefix))
    .map(|prefix| prefix.len())
}

pub(super) fn primary_end(text: &str) -> Option<usize> {
    if continuation_end(text).is_some() {
        return None;
    }

    if text.starts_with("╰─ ") {
        return Some("╰─ ".len());
    }

    if text.starts_with('[') {
        if let Some(end) = text.find(']') {
            if has_user_host_identity(&text[..end])
                && !text[end + 1..].starts_with(['$', '#', '%', '>'])
            {
                return Some(end + 1 + usize::from(text[end + 1..].starts_with(' ')));
            }
        }
    }

    text.char_indices().find_map(|(i, ch)| {
        let icon = i == 0
            && ('\u{e000}'..='\u{f8ff}').contains(&ch)
            && !text[ch.len_utf8()..].chars().any(|ch| ('\u{e000}'..='\u{f8ff}').contains(&ch));

        if !icon && !matches!(ch, '$' | '#' | '%' | '>' | '❯' | '❱' | '➤' | '➜' | '≻' | '»') {
            return None;
        }

        let end = i + ch.len_utf8();

        if !text[end..].starts_with(' ') {
            return None;
        }

        // A decorated prompt can contain more than one marker, e.g. └─➤ $.
        if matches!(ch, '>' | '➤' | '➜' | '❯' | '❱' | '»')
            && ["$ ", "# ", "% "]
                .iter()
                .any(|next| text[end + 1..].starts_with(next))
        {
            return None;
        }

        let prefix = strip_parenthesized_prefixes(text[..i].trim());

        let decoration = prefix == "->"
            || (!prefix.is_empty()
                && prefix
                    .chars()
                    .all(|ch| ch.is_whitespace() || matches!(ch, '└' | '─' | '│' | '➤')));

        let shell = icon
            || matches!(ch, '❯' | '❱' | '≻' | '»')
            || has_user_host_identity(prefix)
            || decoration
            || prefix.starts_with('~')
            || prefix.starts_with('/')
            || (prefix.starts_with('[')
                && prefix.split_whitespace().any(|word| word.starts_with('~') || word.starts_with('/')))
            // Early TTY echo can precede a bracketed shell prompt.
            || (prefix.contains('[') && prefix.ends_with(']'))
            || (prefix.is_empty() && ch != '>' && !text[..i].trim().is_empty())
            || (i == 0 && ch != '>')
            || prefix.starts_with("bash-")
            || prefix.starts_with("zsh-");

        shell.then_some(end + 1)
    })
}

fn strip_parenthesized_prefixes(mut text: &str) -> &str {
    while text.starts_with('(') {
        let mut depth = 0;
        let mut end = None;

        for (i, ch) in text.char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => (),
            }

            if depth == 0 {
                end = Some(i + ch.len_utf8());
                break;
            }
        }

        let Some(end) = end else {
            break;
        };

        text = text[end..].trim_start();
    }

    text
}

pub(super) fn has_user_host_identity(text: &str) -> bool {
    text.split_whitespace().any(|word| {
        let word = word.trim_matches(['[', ']', '(', ')']);
        let identity = word.split_once(':').map_or(word, |(identity, _)| identity);

        let Some((user, host)) = identity.split_once('@') else {
            return false;
        };

        let valid = |part: &str| {
            part.chars().any(char::is_alphanumeric)
                && part
                    .chars()
                    .all(|ch| ch.is_alphanumeric() || matches!(ch, '.' | '_' | '-'))
        };

        valid(user) && valid(host)
    })
}
