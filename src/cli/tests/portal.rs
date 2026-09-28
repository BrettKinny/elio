use super::{Terminal, classify_command, detect_terminal_with, enable_with, parse_parent_process};

fn detect(env: &[(&str, &str)], ancestors: &[&str]) -> Terminal {
    let lookup = |name: &str| {
        env.iter()
            .find_map(|(key, value)| (*key == name).then(|| (*value).to_string()))
    };
    let ancestors = ancestors
        .iter()
        .map(|value| (*value).to_string())
        .collect::<Vec<_>>();
    detect_terminal_with(&lookup, &ancestors)
}

#[test]
fn terminal_specific_environment_markers_are_used_without_ancestry() {
    assert_eq!(
        detect(
            &[("KITTY_WINDOW_ID", "1"), ("TERM_PROGRAM", "WezTerm")],
            &[]
        ),
        Terminal::Kitty
    );
    assert_eq!(
        detect(&[("GNOME_TERMINAL_SCREEN", ":0")], &[]),
        Terminal::Gnome
    );
}

#[test]
fn term_program_and_term_classify_the_supported_set() {
    assert_eq!(
        detect(&[("TERM_PROGRAM", "ghostty")], &[]),
        Terminal::Ghostty
    );
    assert_eq!(detect(&[("TERM", "foot-extra")], &[]), Terminal::Foot);
    assert_eq!(detect(&[("TERM", "xterm-256color")], &[]), Terminal::Xterm);
    assert_eq!(detect(&[("TERM", "xterm-rio")], &[]), Terminal::Rio);
    assert_eq!(
        detect(&[("TERM", "xterm-256color")], &["rio"]),
        Terminal::Rio
    );
}

#[test]
fn nearest_recognized_parent_terminal_takes_priority() {
    assert_eq!(
        detect(
            &[("TERM", "screen")],
            &["bash", "/usr/bin/gnome-terminal-server"]
        ),
        Terminal::Gnome
    );
    assert_eq!(
        detect(&[("KITTY_WINDOW_ID", "1")], &["bash", "/usr/bin/foot"]),
        Terminal::Foot
    );
}

#[test]
fn unknown_terminal_stays_unknown() {
    assert_eq!(
        detect(&[("TERM", "screen")], &["bash", "tmux"]),
        Terminal::Unknown
    );
    assert_eq!(
        classify_command("/usr/bin/wezterm-gui --class elio"),
        Some(Terminal::WezTerm)
    );
}

#[test]
fn enable_is_gated_to_linux_and_freebsd_without_running_detection_elsewhere() {
    for platform in ["macos", "windows", "openbsd"] {
        let error = enable_with(platform, || panic!("unsupported platforms must not detect"))
            .unwrap_err()
            .to_string();
        assert_eq!(
            error,
            "error: `elio portal enable` is only supported on Linux and FreeBSD"
        );
    }

    assert_eq!(
        enable_with("linux", || Terminal::Konsole).unwrap(),
        Terminal::Konsole
    );
    assert_eq!(
        enable_with("freebsd", || Terminal::Xterm).unwrap(),
        Terminal::Xterm
    );
}

#[test]
fn parent_process_parser_accepts_only_complete_ps_records() {
    assert_eq!(
        parse_parent_process("42 /usr/bin/kitty\n"),
        Some(("42".to_string(), "/usr/bin/kitty".to_string()))
    );
    assert_eq!(parse_parent_process("42\n"), None);
    assert_eq!(parse_parent_process(""), None);
}
