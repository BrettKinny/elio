use anyhow::Result;
use std::{env, path::Path};

#[cfg(unix)]
use std::process::Command;

#[cfg(unix)]
const MAX_PARENT_PROCESSES: usize = 6;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PortalCommand {
    Enable,
    Disable,
    Status,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Terminal {
    Kitty,
    Ghostty,
    Foot,
    WezTerm,
    Alacritty,
    Rio,
    Konsole,
    Gnome,
    Xterm,
    Unknown,
}

impl Terminal {
    fn name(self) -> &'static str {
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
            Self::Unknown => "unknown",
        }
    }
}

pub(super) fn execute(command: PortalCommand) -> Result<()> {
    match command {
        PortalCommand::Enable => {
            let (configured, terminal) = enable_with(env::consts::OS, || {
                elio::ensure_portal_terminal(|| configured_name(detect_current_terminal()))
            })?;
            if configured {
                println!("Configured portal terminal: {terminal}");
            } else {
                println!("Portal terminal is already configured: {terminal}");
            }
        }
        PortalCommand::Disable | PortalCommand::Status => {}
    }
    anyhow::bail!("error: portal integration is not implemented yet")
}

fn enable_with<T>(platform: &str, enable: impl FnOnce() -> Result<T>) -> Result<T> {
    if !matches!(platform, "linux" | "freebsd") {
        anyhow::bail!("error: `elio portal enable` is only supported on Linux and FreeBSD")
    }
    enable()
}

fn configured_name(terminal: Terminal) -> Option<&'static str> {
    (terminal != Terminal::Unknown).then(|| terminal.name())
}

fn detect_current_terminal() -> Terminal {
    detect_terminal_with(&real_env_lookup, &parent_commands())
}

fn detect_terminal_with(
    env_lookup: &impl Fn(&str) -> Option<String>,
    ancestors: &[String],
) -> Terminal {
    if let Some(terminal) = ancestors
        .iter()
        .find_map(|command| classify_command(command))
    {
        return terminal;
    }

    let has = |name| env_lookup(name).is_some();
    if has("KITTY_WINDOW_ID") {
        return Terminal::Kitty;
    }
    if has("WEZTERM_PANE") {
        return Terminal::WezTerm;
    }
    if has("ALACRITTY_SOCKET") {
        return Terminal::Alacritty;
    }
    if has("KONSOLE_DBUS_SESSION") || has("KONSOLE_DBUS_SERVICE") || has("KONSOLE_DBUS_WINDOW") {
        return Terminal::Konsole;
    }
    if has("GNOME_TERMINAL_SCREEN") || has("GNOME_TERMINAL_SERVICE") {
        return Terminal::Gnome;
    }

    let term_program = env_lookup("TERM_PROGRAM")
        .unwrap_or_default()
        .to_ascii_lowercase();
    if let Some(terminal) = classify_name(&term_program) {
        return terminal;
    }

    let term = env_lookup("TERM").unwrap_or_default().to_ascii_lowercase();
    if term.contains("xterm-kitty") {
        return Terminal::Kitty;
    }
    if term.contains("ghostty") {
        return Terminal::Ghostty;
    }
    if term.contains("wezterm") {
        return Terminal::WezTerm;
    }
    if term.contains("alacritty") {
        return Terminal::Alacritty;
    }
    if matches!(term.as_str(), "rio" | "xterm-rio") {
        return Terminal::Rio;
    }
    if matches!(term.as_str(), "foot" | "foot-extra") {
        return Terminal::Foot;
    }
    if term.starts_with("xterm") {
        return Terminal::Xterm;
    }

    Terminal::Unknown
}

fn classify_command(command: &str) -> Option<Terminal> {
    let command = command.split_whitespace().next()?;
    let name = Path::new(command)
        .file_name()?
        .to_string_lossy()
        .to_ascii_lowercase();
    classify_name(&name)
}

fn classify_name(name: &str) -> Option<Terminal> {
    match name {
        "kitty" => Some(Terminal::Kitty),
        "ghostty" => Some(Terminal::Ghostty),
        "foot" | "footclient" => Some(Terminal::Foot),
        "wezterm" | "wezterm-gui" => Some(Terminal::WezTerm),
        "alacritty" => Some(Terminal::Alacritty),
        "rio" => Some(Terminal::Rio),
        "konsole" => Some(Terminal::Konsole),
        "gnome-terminal" | "gnome-terminal-server" => Some(Terminal::Gnome),
        "xterm" => Some(Terminal::Xterm),
        _ => None,
    }
}

fn real_env_lookup(name: &str) -> Option<String> {
    env::var_os(name).map(|value| value.to_string_lossy().into_owned())
}

#[cfg(unix)]
fn parent_commands() -> Vec<String> {
    // This is a best-effort signal that takes precedence over inherited terminal
    // environment. A missing or incompatible `ps` yields no ancestry.
    let mut commands = Vec::new();
    let mut pid = unsafe { libc::getppid() }.to_string();
    for _ in 0..MAX_PARENT_PROCESSES {
        let Some((parent, command)) = parent_process(&pid) else {
            return Vec::new();
        };
        commands.push(command);
        if parent == "0" || parent == pid {
            break;
        }
        pid = parent;
    }
    commands
}

#[cfg(not(unix))]
fn parent_commands() -> Vec<String> {
    Vec::new()
}

#[cfg(unix)]
fn parent_process(pid: &str) -> Option<(String, String)> {
    let output = Command::new("ps")
        .args(["-p", pid, "-o", "ppid=", "-o", "comm="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let output = String::from_utf8(output.stdout).ok()?;
    parse_parent_process(&output)
}

#[cfg(any(unix, test))]
fn parse_parent_process(output: &str) -> Option<(String, String)> {
    let mut fields = output.split_whitespace();
    Some((fields.next()?.to_string(), fields.next()?.to_string()))
}

#[cfg(test)]
#[path = "tests/portal.rs"]
mod tests;
