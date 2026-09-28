use serde::Deserialize;
use std::{ffi::OsString, path::Path};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum TerminalAdapter {
    Kitty,
    Ghostty,
    Foot,
    #[serde(rename = "wezterm")]
    WezTerm,
    Alacritty,
    Rio,
    Konsole,
    #[serde(rename = "gnome-terminal")]
    Gnome,
    Xterm,
}

impl TerminalAdapter {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "kitty" => Some(Self::Kitty),
            "ghostty" => Some(Self::Ghostty),
            "foot" => Some(Self::Foot),
            "wezterm" => Some(Self::WezTerm),
            "alacritty" => Some(Self::Alacritty),
            "rio" => Some(Self::Rio),
            "konsole" => Some(Self::Konsole),
            "gnome-terminal" => Some(Self::Gnome),
            "xterm" => Some(Self::Xterm),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Kitty => "kitty",
            Self::Ghostty => "ghostty",
            Self::Foot => "foot",
            Self::WezTerm => "wezterm",
            Self::Alacritty => "alacritty",
            Self::Rio => "rio",
            Self::Konsole => "konsole",
            Self::Gnome => "gnome-terminal",
            Self::Xterm => "xterm",
        }
    }

    /// Build terminal arguments for a portal chooser child. This performs no
    /// process spawning or executable resolution.
    pub fn chooser_args(self, child: &[OsString]) -> Vec<OsString> {
        let mut args = match self {
            Self::Kitty | Self::Foot => Vec::new(),
            Self::Ghostty => vec!["-e".into()],
            Self::WezTerm => vec!["start".into(), "--".into()],
            Self::Alacritty => vec!["--command".into()],
            Self::Rio => vec!["--command".into()],
            Self::Konsole => vec!["-e".into()],
            Self::Gnome => vec!["--".into()],
            Self::Xterm => vec!["-e".into()],
        };
        args.extend_from_slice(child);
        args
    }
}

pub fn detect_with(
    env_lookup: &impl Fn(&str) -> Option<String>,
    ancestors: &[String],
) -> Option<TerminalAdapter> {
    if let Some(terminal) = ancestors
        .iter()
        .find_map(|command| classify_command(command))
    {
        return Some(terminal);
    }

    let has = |name| env_lookup(name).is_some();
    if has("KITTY_WINDOW_ID") {
        return Some(TerminalAdapter::Kitty);
    }
    if has("WEZTERM_PANE") {
        return Some(TerminalAdapter::WezTerm);
    }
    if has("ALACRITTY_SOCKET") {
        return Some(TerminalAdapter::Alacritty);
    }
    if has("KONSOLE_DBUS_SESSION") || has("KONSOLE_DBUS_SERVICE") || has("KONSOLE_DBUS_WINDOW") {
        return Some(TerminalAdapter::Konsole);
    }
    if has("GNOME_TERMINAL_SCREEN") || has("GNOME_TERMINAL_SERVICE") {
        return Some(TerminalAdapter::Gnome);
    }

    let term_program = env_lookup("TERM_PROGRAM")
        .unwrap_or_default()
        .to_ascii_lowercase();
    if let Some(terminal) = classify_name(&term_program) {
        return Some(terminal);
    }

    let term = env_lookup("TERM").unwrap_or_default().to_ascii_lowercase();
    if term.contains("xterm-kitty") {
        return Some(TerminalAdapter::Kitty);
    }
    if term.contains("ghostty") {
        return Some(TerminalAdapter::Ghostty);
    }
    if term.contains("wezterm") {
        return Some(TerminalAdapter::WezTerm);
    }
    if term.contains("alacritty") {
        return Some(TerminalAdapter::Alacritty);
    }
    if matches!(term.as_str(), "rio" | "xterm-rio") {
        return Some(TerminalAdapter::Rio);
    }
    if matches!(term.as_str(), "foot" | "foot-extra") {
        return Some(TerminalAdapter::Foot);
    }
    if term.starts_with("xterm") {
        return Some(TerminalAdapter::Xterm);
    }

    None
}

pub fn classify_command(command: &str) -> Option<TerminalAdapter> {
    let command = command.split_whitespace().next()?;
    let name = Path::new(command)
        .file_name()?
        .to_string_lossy()
        .to_ascii_lowercase();
    classify_name(&name)
}

fn classify_name(name: &str) -> Option<TerminalAdapter> {
    match name {
        "kitty" => Some(TerminalAdapter::Kitty),
        "ghostty" => Some(TerminalAdapter::Ghostty),
        "foot" | "footclient" => Some(TerminalAdapter::Foot),
        "wezterm" | "wezterm-gui" => Some(TerminalAdapter::WezTerm),
        "alacritty" => Some(TerminalAdapter::Alacritty),
        "rio" => Some(TerminalAdapter::Rio),
        "konsole" => Some(TerminalAdapter::Konsole),
        "gnome-terminal" | "gnome-terminal-server" => Some(TerminalAdapter::Gnome),
        "xterm" => Some(TerminalAdapter::Xterm),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn child() -> Vec<OsString> {
        [
            "/usr/bin/elio",
            "--portal-chooser",
            "--socket",
            "/tmp/request.sock",
        ]
        .map(OsString::from)
        .to_vec()
    }

    #[test]
    fn chooser_args_use_direct_terminal_argv() {
        let cases = [
            (TerminalAdapter::Kitty, &[][..]),
            (TerminalAdapter::Ghostty, &["-e"][..]),
            (TerminalAdapter::Foot, &[][..]),
            (TerminalAdapter::WezTerm, &["start", "--"][..]),
            (TerminalAdapter::Alacritty, &["--command"][..]),
            (TerminalAdapter::Rio, &["--command"][..]),
            (TerminalAdapter::Konsole, &["-e"][..]),
            (TerminalAdapter::Gnome, &["--"][..]),
            (TerminalAdapter::Xterm, &["-e"][..]),
        ];

        for (terminal, prefix) in cases {
            assert_eq!(
                terminal.chooser_args(&child()),
                prefix
                    .iter()
                    .copied()
                    .chain([
                        "/usr/bin/elio",
                        "--portal-chooser",
                        "--socket",
                        "/tmp/request.sock"
                    ])
                    .map(OsString::from)
                    .collect::<Vec<_>>(),
            );
        }
    }

    #[test]
    fn supported_config_names_round_trip() {
        for name in [
            "kitty",
            "ghostty",
            "foot",
            "wezterm",
            "alacritty",
            "rio",
            "konsole",
            "gnome-terminal",
            "xterm",
        ] {
            assert_eq!(TerminalAdapter::from_name(name).unwrap().name(), name);
        }
    }
}
