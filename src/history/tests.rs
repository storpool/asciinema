use std::time::Duration;

use crate::asciicast::{Asciicast, Event, EventData, Header, Version};

fn history(output: &[&str]) -> Vec<String> {
    events(
        Header::default(),
        output
            .iter()
            .map(|text| EventData::Output((*text).to_owned()))
            .collect(),
    )
}

fn events(header: Header, data: Vec<EventData>) -> Vec<String> {
    super::extract(Asciicast {
        version: Version::Three,
        header,
        events: Box::new(data.into_iter().map(|data| {
            Ok(Event {
                time: Duration::ZERO,
                data,
            })
        })),
    })
    .unwrap()
}

#[test]
fn editing_and_submission_from_the_middle_of_the_buffer() {
    assert_eq!(
        history(&[
            "$ ",
            "echo bad tail",
            "\x1b[8D\x1b[3P\x1b[4@good",
            "\r\n",
            "good tail\r\n$ ",
            "echo old",
            "\r\x1b[2C\x1b[Kecho new",
            "\r\n",
        ]),
        ["echo good tail", "echo new"]
    );
}

#[test]
fn cancelled_and_unfinished_edits_are_not_submissions() {
    assert_eq!(
        history(&[
            "\x1b[?2004h$ ",
            "echo abandoned",
            "^C\r\n\x1b[?2004l",
            "\x1b[?2004h$ ",
            "echo done",
            "\r\n\x1b[?2004l",
            "done\r\n",
            "\x1b[?2004h$ ",
            "echo recalled",
            "\r\x1b[2C\x1b[K",
            "echo unfinished",
        ]),
        ["echo done"]
    );

    assert!(history(&["$ echo \\\r\n> unfinished"]).is_empty());
    assert!(history(&["$ echo \\\r\n> cancelled^C\r\n$ "]).is_empty());
    assert!(history(&["$ echo \\\r\n"]).is_empty());
    assert!(history(&["$ echo 'unfinished\r\n"]).is_empty());
}

#[test]
fn duplicate_submissions_and_coalesced_output() {
    let text = "\x1b[?2004h$ true\r\n\x1b[?2004l\x1b[?2004h$ true\r\n\x1b[?2004l";
    assert_eq!(history(&[text]), ["true", "true"]);

    assert_eq!(
        history(&["$ ", "true", "\r\n$ ", "true", "\r\n"]),
        ["true", "true"]
    );
}

#[test]
fn continuation_prompts_join_quotes_blocks_and_here_documents() {
    assert_eq!(
        history(&[
            "$ ",
            "printf '%s' 'first  ",
            "\r\n> ",
            "second'",
            "\r\nfirst  second\r\n$ ",
            "for x in one two; do",
            "\r\n> ",
            "  echo \"$x\"",
            "\r\n> ",
            "done",
            "\r\none\r\ntwo\r\n$ ",
            "cat <<'EOF'",
            "\r\n> ",
            "  body  ",
            "\r\n> ",
            "EOF",
            "\r\n  body  \r\n$ ",
        ]),
        [
            "printf '%s' 'first  \nsecond'",
            "for x in one two; do\n  echo \"$x\"\ndone",
            "cat <<'EOF'\n  body  \nEOF"
        ]
    );
}

#[test]
fn fish_multiline_uses_editor_indentation() {
    // Reduced Fish 4.2 output, with shell-integration markers removed.
    assert_eq!(
        history(&[
            "\x1b[?2004hfish@host ~> \r\x1b[13C",
            "printf '%s\\n' 'first  \r\n\x1b[K\x1b[13C\x1b[Aprintf '%s\\n' 'first  \r\r\n\x1b[13C",
            "\x1b[?2004lsecond'\r\x1b[20C\r\nfirst  \r\nsecond\r\n",
            "\x1b[?2004hfish@host ~> \r\x1b[13C",
            "begin\r\n\x1b[K\x1b[17C\x1b[A\x1b[4Dbegin\r\r\n\x1b[17C",
            "  echo hello\r\n\x1b[K\x1b[17C\x1b[A  echo hello\r\r\n\x1b[17C",
            "\x1b[?2004l\x1b[4Dend\x1b[K\r\x1b[16C\r\nhello\r\n",
            "\x1b[?2004hfish@host ~> ",
        ]),
        [
            "printf '%s\\n' 'first  \nsecond'",
            "begin\n  echo hello\nend"
        ]
    );
}

#[test]
fn shell_transitions_and_password_prompts() {
    assert_eq!(
        history(&[
            "local@host:~$ ",
            "ssh remote",
            "\r\nPassword: ",
            "\r\nremote@host:~$ ",
            "sudo -s",
            "\r\n[sudo] password for user: ",
            "\r\n# ",
            "id",
            "\r\nuid=0(root)\r\n# ",
            "su user",
            "\r\n$ ",
            "pwd",
            "\r\n/home/user\r\n$ ",
            "exit",
            "\r\nexit\r\n# ",
            "exit",
            "\r\nexit\r\nremote@host:~$ ",
        ]),
        [
            "ssh remote",
            "sudo -s",
            "id",
            "su user",
            "pwd",
            "exit",
            "exit"
        ]
    );
}

#[test]
fn remote_shell_can_disable_bracketed_paste() {
    assert_eq!(
        history(&[
            "\x1b[?2004h❯ ",
            "ssh remote",
            "\x1b[?2004l\r\n",
            "remote@host:~$ ",
            "echo \\",
            "\r\n> ",
            "  hello",
            "\r\nhello\r\nremote@host:~$ ",
        ]),
        ["ssh remote", "echo \\\n  hello"]
    );
}

#[test]
fn output_is_not_a_continuation_prompt() {
    assert_eq!(
        history(&["$ npm install\r\n> build\r\n> cargo build\r\n$ "]),
        ["npm install"]
    );
}

#[test]
fn metadata_and_input_are_not_recognition_signals() {
    assert_eq!(
        events(
            Header::default(),
            vec![
                EventData::Marker("$ invented\r\n".to_owned()),
                EventData::Input("secret\r".to_owned()),
                EventData::Other('z', "$ invented\r\n".to_owned()),
                EventData::Output("$ real\r\n".to_owned()),
                EventData::Exit(0),
            ]
        ),
        ["real"]
    );
}

#[test]
fn final_quoted_and_escaped_spaces_are_preserved() {
    assert_eq!(
        history(&[
            "$ ",
            "printf '%s' 'a  '  ",
            "\r\na  \r\n$ ",
            "printf '%s' a\\ ",
            "\r\na \r\n$ ",
        ]),
        ["printf '%s' 'a  '", "printf '%s' a\\ "]
    );
}

#[test]
fn comments_and_here_doc_bodies_are_not_unclosed_shell_quotes() {
    assert_eq!(
        history(&[
            "$ ",
            "echo ok # user's comment",
            "\r\nok\r\n$ ",
            "cat <<'EOF'",
            "\r\n> ",
            "it's literal  ",
            "\r\n> ",
            "EOF",
            "\r\nit's literal  \r\n$ ",
        ]),
        [
            "echo ok # user's comment",
            "cat <<'EOF'\nit's literal  \nEOF"
        ]
    );
}

#[test]
fn application_prompts_and_bracketed_paste_are_not_shell_commands() {
    assert_eq!(
        history(&[
            "\x1b[?2004huser@host:~$ python\r\n\x1b[?2004l",
            "\x1b[?2004h>>> print('hi')\r\n\x1b[?2004lhi\r\n",
            "\x1b[?2004h>>> exit()\r\n\x1b[?2004l",
            "\x1b[?2004huser@host:~$ npm install\r\n\x1b[?2004l",
            "> build\r\n> cargo build\r\n",
            "\x1b[?2004huser@host:~$ vim file\r\n\x1b[?2004l",
            "\x1b[?1049h\x1b[?2004h$ fake\r\n\x1b[?2004l\x1b[?1049l",
            "\x1b[?2004huser@host:~$ ",
        ]),
        ["python", "npm install", "vim file"]
    );
}

#[test]
fn completion_without_bracketed_paste_is_not_a_submission() {
    assert_eq!(
        history(&[
            "user@host:~$ ",
            "cat fi",
            "\r\nfile1  file2\r\nuser@host:~$ cat fi",
            "le1",
            "\r\ncontents\r\nuser@host:~$ ",
        ]),
        ["cat file1"]
    );
}

#[test]
fn resizes_preserve_soft_wrapped_commands_and_wide_characters() {
    assert_eq!(
        events(
            Header {
                term_cols: 12,
                term_rows: 3,
                ..Header::default()
            },
            vec![
                EventData::Output("$ printf '界 abcdefghijklmnop'".to_owned()),
                EventData::Resize(9, 4),
                EventData::Resize(16, 3),
                EventData::Output("\r\n".to_owned()),
            ]
        ),
        ["printf '界 abcdefghijklmnop'"]
    );
}

#[test]
fn escape_sequences_can_cross_any_event_boundary() {
    let text = "\x1bkhidden $ title\x1b\\\x1b[?2004huser@host:~$ echo yes\r\n\x1b[?2004lyes\r\n";

    for split in text.char_indices().map(|(i, _)| i) {
        assert_eq!(
            history(&[&text[..split], &text[split..]]),
            ["echo yes"],
            "split at {split}"
        );
    }
}

#[test]
fn explicit_exit_and_generated_eof_message() {
    assert!(history(&["$ ", "exit\r\n"]).is_empty());
    assert_eq!(history(&["$ ", "exit", "\r\nexit\r\n"]), ["exit"]);
    assert_eq!(history(&["$ exit\r\nexit\r\n"]), ["exit"]);
}

#[test]
fn early_echo_before_a_bracketed_prompt() {
    assert_eq!(
        history(&[
            "c",
            "[\x1b[31mroot\x1b[m][host][~]# c",
            "d /tmp/",
            "\r\n",
            "[root][host][/tmp]# ",
            "pwd",
            "\r\n/tmp\r\n[root][host][/tmp]# ",
            "exit\r\n",
        ]),
        ["cd /tmp/", "pwd"]
    );
}

#[test]
fn decorated_path_and_branch_prompts_use_the_final_marker() {
    assert_eq!(
        history(&[
            "\x1b[?2004hproject [main] $ ",
            "echo one",
            "\r\n\x1b[?2004lone\r\n",
            "\x1b[?2004h└─➤ $ ",
            "echo two",
            "\r\n\x1b[?2004ltwo\r\n",
            "\x1b[?2004h~ $ ",
            "echo three",
            "\r\n\x1b[?2004lthree\r\n",
            "\x1b[?2004h[I] #host ~/work $ ",
            "echo four",
            "\r\n\x1b[?2004lfour\r\n",
            "\x1b[?2004h└─$ ",
            "exit",
            "\x1b[?2004l\r\n",
        ]),
        ["echo one", "echo two", "echo three", "echo four", "exit"]
    );
}

#[test]
fn punctuation_and_application_arrow_prompts_are_not_shell_prompts() {
    assert_eq!(
        history(&[
            "\x1b[?2004h~ $ ",
            "gdb",
            "\r\n\x1b[?2004l",
            "\x1b[?2004hgef➤  start\r\n\x1b[?2004l",
            "  % Total    % Received % Xferd\r\n",
            "   #  Name             Rank\r\n",
            "[*] A connection opened (host -> remote)\r\n",
            "@@*/@@@*(&,@#&# @(@*,@/@,@&*@*@\r\n",
            "\x1b[?2004h~ $ ",
            "echo ok",
            "\r\n\x1b[?2004lok\r\n",
        ]),
        ["gdb", "echo ok"]
    );
}

#[test]
fn right_prompt_at_the_margin_is_not_part_of_the_command() {
    assert_eq!(
        events(
            Header {
                term_cols: 12,
                term_rows: 4,
                ..Header::default()
            },
            [
                "\x1b[?2004h» \r\x1b[8C1234\r\x1b[2C",
                "ab\x1b[2D\x1b[K\r\x1b[8C1234\r\x1b[2C",
                "true",
                "\r\n\x1b[?2004l",
                "\x1b[?2004h» \r\x1b[8C5678\r\x1b[2C",
                "echo long-command",
                "\r\n\x1b[?2004llong-command\r\n",
            ]
            .into_iter()
            .map(|text| EventData::Output(text.to_owned()))
            .collect(),
        ),
        ["true", "echo long-command"]
    );

    assert_eq!(
        history(&["$ ", "echo        spaced", "\r\x1b[6C\r\n"]),
        ["echo        spaced"]
    );
}

#[test]
fn reverse_search_can_submit_by_moving_down_without_lf() {
    assert_eq!(
        history(&[
            "\x1b[?2004h❯ ",
            "echo found",
            "\r\x1b[2C",
            "\x1b[?2004l\x1b[1B\r",
            "found\r\n",
            "\x1b[?2004h❯ ",
        ]),
        ["echo found"]
    );
}

#[test]
fn trailing_pasted_newlines_are_submission_separators() {
    assert_eq!(
        history(&[
            "\x1b[?2004h$ ",
            "echo one\r\necho two\r\n\r\n\x1b[?2004l",
            "one\r\ntwo\r\n\x1b[?2004h$ ",
        ]),
        ["echo one\necho two"]
    );

    assert_eq!(
        super::command_tail("printf '%s' 'line\n'  ").trimmed,
        "printf '%s' 'line\n'"
    );
}

#[test]
fn execution_output_cannot_reopen_a_continuation() {
    assert_eq!(
        history(&[
            "host@remote:~$ ",
            "curl -v example.org\r\n",
            "* Connected\r\n> ",
            "User-Agent: example\r\n> Accept: */*\r\n",
            "host@remote:~$ ",
        ]),
        ["curl -v example.org"]
    );
}

#[test]
fn execution_menus_traces_and_symbols_are_not_shell_editors() {
    assert_eq!(
        history(&[
            "\x1b[?2004huser@host:~$ ",
            "tool\r\n\x1b[?2004l",
            "Select an object\r\n❯ folder/\r\n  other/\r\n",
            "\x1b[2A❯ ",
            "other/\r\n",
            "$ ",
            "trace(\r\n",
            "\x1b[?2004hgef➤  next\r\n\x1b[?2004l",
            "0x00001234 <printf@plt+4> add r12, pc, #0\r\n",
            "0x00001234 <entry@plt> ret\r\n",
            "\x1b[?2004huser@host:~$ ",
            "echo ok\r\n\x1b[?2004lok\r\n",
        ]),
        ["tool", "echo ok"]
    );
}

#[test]
fn reaching_the_right_prompt_is_not_overwriting_it() {
    assert_eq!(
        events(
            Header {
                term_cols: 12,
                term_rows: 4,
                ..Header::default()
            },
            [
                "\x1b[?2004h$ \r\x1b[8CRP\r\x1b[2C",
                "echo x\r\x1b[8CRP\r\x1b[8C\r\n\x1b[?2004l",
                "\x1b[?2004h$ \r\x1b[8CRP\r\x1b[2C",
                "falsex\r\x1b[8CRP\r\x1b[2C\x1b[K",
                "true\r\x1b[8CRP\r\x1b[6C\r\n\x1b[?2004l",
                "\x1b[?2004h$ \r\x1b[8CRP\r\x1b[2C",
                "echo x\r\x1b[8CRP\r\x1b[7C\x1b[C0\x1b[K\r\x1b[9C\r\n\x1b[?2004l",
                "\x1b[?2004h$ \r\x1b[8CRP\r\x1b[2C",
                "echo x0\x1b[K\r\x1b[2Ctrue\x1b[K\r\x1b[8CRP\r\x1b[6C\r\n\x1b[?2004l",
            ]
            .into_iter()
            .map(|text| EventData::Output(text.to_owned()))
            .collect(),
        ),
        ["echo x", "true", "echo x0", "true"]
    );
}

#[test]
fn prompt_families_and_observed_arrow_prefixes() {
    assert_eq!(
        history(&[
            "\x1b[?2004h (feature/example)# echo one\r\n\x1b[?2004lone\r\n",
            "-> % \x1b[?2004hecho two\x1b[?2004l\r\n",
            "\x1b[?2004h((venv) ) ~ $ echo three\r\n\x1b[?2004lthree\r\n",
            "\x1b[?2004h[user@host:~]echo four\r\n\x1b[?2004lfour\r\n",
            "\x1b[?2004h❰user❙~/project❱✔≻ echo five\r\n\x1b[?2004lfive\r\n",
            "➜  project git:(main) ✗ \x1b[?2004h  echo 'six'\x1b[?2004l\r\n",
            "➜  other \x1b[?2004hecho seven\x1b[?2004l\r\n",
            "➜ \x1b[?2004hecho eight\x1b[?2004l\r\n",
        ]),
        [
            "echo one",
            "echo two",
            "echo three",
            "echo four",
            "echo five",
            "  echo 'six'",
            "echo seven",
            "echo eight"
        ]
    );
}

#[test]
fn transient_prompt_redraws_can_move_up_and_reopen_the_editor() {
    assert_eq!(
        history(&[
            "status\r\ncustom prompt \x1b[?2004hfirst",
            "\x1b[?2004l\r\x1b[A\x1b[J❯ first\r\n",
            "status\r\ncustom prompt \x1b[?2004hsecond",
            "\x1b[?2004l\r\x1b[A\x1b[J❯ second\r\n",
            "status\r\n╰─ \x1b[?1h\x1b[?2004hthird",
            "\x1b[?2004l\r\x1b[A\x1b[?2004h\x1b[J\u{e285} third",
            "\x1b[?1l\x1b[?2004l\r\n",
        ]),
        ["first", "second", "third"]
    );
}

#[test]
fn simulated_arrow_prompt_typing_requires_an_editor() {
    assert_eq!(
        history(&[
            "bash-3.2$ ./demo.sh\r\n",
            "➜ project ",
            "echo simulated",
            "\r\nsimulated\r\n",
            "bash-3.2$ ",
            "echo real\r\n",
        ]),
        ["./demo.sh", "echo real"]
    );
}

#[test]
fn remote_inline_eof_messages_are_language_independent() {
    for message in ["注销", "déconnexion", "logout"] {
        assert!(history(&[
            "user@remote:~$ ",
            &format!("{message}\r\n"),
            "Connection to remote closed.\r\nlocal@host:~$ ",
        ])
        .is_empty());
    }

    assert_eq!(
        history(&[
            "user@remote:~$ ",
            "logout",
            "\r\nConnection to remote closed.\r\nlocal@host:~$ ",
        ]),
        ["logout"]
    );

    assert_eq!(
        history(&[
            "user@remote:~$ ",
            "uptime\r\nup 3 days\r\nConnection to remote closed.\r\n",
        ]),
        ["uptime"]
    );
}

#[test]
fn editor_confirmation_takes_precedence_over_incomplete_quote_inference() {
    for command in [
        "echo $'it\\'s fine'",
        "  echo $'it\\'s fine' escaped\\ ",
        "echo \"${value:-\"don't\"}\"",
    ] {
        for finish in ["\x1b[?2004l\r\n", "\r\n\x1b[?2004l"] {
            assert_eq!(history(&["\x1b[?2004h$ ", command, finish]), [command]);
        }
    }

    for continuation in ["\x1b[?2004h> ", "quote> \x1b[?2004h"] {
        assert!(history(&[
            "\x1b[?1h\x1b[?2004h$ echo 'unfinished\x1b[?1;2004l\r\n",
            continuation,
            "still editing",
        ])
        .is_empty());

        assert!(history(&[
            "\x1b[?1h\x1b[?2004h$ echo 'cancelled\x1b[?1;2004l\r\n",
            continuation,
            "\x1b[?1h\x1b[?2004l\r\n",
        ])
        .is_empty());
    }
}

#[test]
fn grouped_editor_mode_changes_use_the_updated_terminal_state() {
    for reset in ["\x1b[?1l\x1b[?2004l", "\x1b[?1;2004l", "\x1b[?2004;1l"] {
        for finish in [format!("{reset}\r\n"), format!("\r\n{reset}")] {
            assert_eq!(
                history(&["\x1b[?1h\x1b[?2004h$ ", "echo accepted", &finish]),
                ["echo accepted"],
                "editor close {finish:?}"
            );
        }
    }

    // Retaining application-cursor mode on close is still Zsh cancellation.
    for finish in ["\x1b[?2004l\r\n", "\r\n\x1b[?2004l"] {
        assert!(history(&["\x1b[?1h\x1b[?2004h$ ", "echo cancelled", finish]).is_empty());
    }

    for modes in ["1049;2004", "2004;1049"] {
        assert_eq!(
            history(&[
                "\x1b[?2004h$ viewer\r\n\x1b[?2004l",
                &format!("\x1b[?{modes}h$ application input\r\n\x1b[?{modes}l"),
                "\x1b[?2004h$ echo returned\r\n\x1b[?2004l",
            ]),
            ["viewer", "echo returned"]
        );
    }
}

#[test]
fn wide_characters_do_not_create_a_false_right_prompt() {
    // Submit from the middle of the final word, after four two-column characters.
    for prompt in ["\x1b[?2004h$ ", "\x1b[?2004h$ \r\x1b[70CRP\r\x1b[2C"] {
        assert_eq!(
            history(&[prompt, "echo 界界界界 tail", "\r\x1b[18C\r\n\x1b[?2004l"]),
            ["echo 界界界界 tail"]
        );
    }
}

#[test]
fn repeated_paste_mode_commands_preserve_submissions() {
    for enable in ["\x1b[?2004h", "\x1b[?2004h\x1b[?2004h", "\x1b[?2004;2004h"] {
        for disable in ["\x1b[?2004l", "\x1b[?2004l\x1b[?2004l", "\x1b[?2004;2004l"] {
            let text = format!("{enable}$ true\r\n{disable}{enable}$ true\r\n{disable}");
            assert_eq!(history(&[&text]), ["true", "true"]);
        }
    }
}

#[test]
fn terminal_resets_preserve_history_and_discard_unfinished_edits() {
    for unfinished in ["echo abandoned", "echo 'unfinished\r\n> more"] {
        assert_eq!(
            history(&[
                "\x1b[?2004h$ echo accepted\r\n\x1b[?2004l",
                "\x1bc\x1b[?2004h$ ",
                unfinished,
                "\x1bc$ echo returned\r\n",
            ]),
            ["echo accepted", "echo returned"]
        );
    }

    assert!(history(&["\x1b[?2004h$ echo unfinished\r\n\x1bc"]).is_empty());

    assert_eq!(
        history(&["\x1b[?2004h$ echo kept\x1b[!p\r\n\x1b[?2004l"]),
        ["echo kept"]
    );
}

#[test]
fn legacy_titles_cannot_supply_commands() {
    for end in ["\x1b\\", "\u{9c}"] {
        let text = format!(
            "\x1b[?2004h$ echo kept\x1bktitle 日 $ echo fake\r\n\x07ignored{end}\r\n\x1b[?2004l"
        );

        for split in text.char_indices().map(|(index, _)| index) {
            assert_eq!(history(&[&text[..split], &text[split..]]), ["echo kept"]);
        }
    }
}

#[test]
fn long_output_events_preserve_multiline_commands_across_eviction() {
    let lines: Vec<_> = (0..150).map(|index| format!("line {index}")).collect();
    let command = format!("printf '%s\\n' '{}'", lines.join("\n"));

    let text = format!(
        "\x1b[?2004h$ printf '%s\\n' '{}'\r\n\x1b[?2004l",
        lines.join("\r\n> ")
    );

    for fragments in [vec![text.as_str()], text.split_inclusive('\n').collect()] {
        let mut data: Vec<_> = fragments
            .into_iter()
            .map(|text| EventData::Output(text.to_owned()))
            .collect();

        data.push(EventData::Resize(40, 6));

        data.push(EventData::Output(
            "\x1b[?2004h$ pwd\r\n\x1b[?2004l".to_owned(),
        ));

        assert_eq!(
            events(Header::default(), data),
            [command.clone(), "pwd".to_owned()]
        );
    }
}
