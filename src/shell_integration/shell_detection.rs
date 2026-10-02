use anyhow::Result;
use std::{env, path::Path};

pub(crate) const SUPPORTED_SHELLS: &str = "bash, zsh, fish, nu, pwsh";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Shell {
    Bash,
    Zsh,
    Fish,
    Nu,
    Pwsh,
}

impl Shell {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "bash" => Ok(Self::Bash),
            "zsh" => Ok(Self::Zsh),
            "fish" => Ok(Self::Fish),
            "nu" | "nushell" => Ok(Self::Nu),
            "pwsh" | "powershell" => Ok(Self::Pwsh),
            shell => Err(format!(
                "error: unsupported shell '{shell}'

supported shells: {SUPPORTED_SHELLS}"
            )),
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Zsh => "zsh",
            Self::Fish => "fish",
            Self::Nu => "nu",
            Self::Pwsh => "pwsh",
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ShellIntegrationAction {
    Install,
    Uninstall,
}

impl ShellIntegrationAction {
    fn command(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Uninstall => "uninstall",
        }
    }

    #[cfg(any(unix, test))]
    fn active_shell_description(self) -> &'static str {
        match self {
            Self::Install => "installs integration for",
            Self::Uninstall => "removes integration from",
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum ShellDetection {
    #[cfg(any(unix, test))]
    Supported(Shell),
    #[cfg(any(unix, test))]
    Unsupported(String),
    Unknown,
}

pub(crate) fn detect_shell(action: ShellIntegrationAction) -> Result<Shell> {
    match detect_parent_shell() {
        #[cfg(any(unix, test))]
        ShellDetection::Supported(shell) => Ok(shell),
        #[cfg(any(unix, test))]
        ShellDetection::Unsupported(shell) => Err(anyhow::anyhow!(
            unsupported_active_shell_message(action, &shell)
        )),
        ShellDetection::Unknown => detect_shell_from_environment(action),
    }
}

fn detect_shell_from_environment(action: ShellIntegrationAction) -> Result<Shell> {
    if let Some(shell) = env::var("SHELL").ok().filter(|shell| !shell.is_empty()) {
        let name = shell_name_from_command(&shell).unwrap_or(shell);
        return Shell::parse(&name).map_err(anyhow::Error::msg);
    }

    if running_under_powershell() {
        return Ok(Shell::Pwsh);
    }

    Err(anyhow::anyhow!(
        "error: could not detect your shell from the parent process or $SHELL\n\n{}",
        explicit_shell_guidance(action)
    ))
}

/// PowerShell exports `PSModulePath` to child processes, so finding it without
/// `$SHELL` (the usual Windows case) means the parent is almost certainly PowerShell.
fn running_under_powershell() -> bool {
    env::var_os("PSModulePath").is_some_and(|value| !value.is_empty())
}

#[cfg(unix)]
fn detect_parent_shell() -> ShellDetection {
    let parent_pid = unsafe { libc::getppid() }.to_string();
    let output = std::process::Command::new("ps")
        .args(["-p", &parent_pid, "-o", "comm="])
        .output()
        .ok();
    let Some(output) = output else {
        return ShellDetection::Unknown;
    };

    if !output.status.success() {
        return ShellDetection::Unknown;
    }

    let Ok(command) = String::from_utf8(output.stdout) else {
        return ShellDetection::Unknown;
    };

    detect_shell_from_command(&command)
}

#[cfg(not(unix))]
fn detect_parent_shell() -> ShellDetection {
    ShellDetection::Unknown
}

#[cfg(any(unix, test))]
pub(super) fn detect_shell_from_command(command: &str) -> ShellDetection {
    let Some(name) = shell_name_from_command(command) else {
        return ShellDetection::Unknown;
    };

    match Shell::parse(&name) {
        Ok(shell) => ShellDetection::Supported(shell),
        Err(_) if is_known_unsupported_shell(&name) => ShellDetection::Unsupported(name),
        Err(_) => ShellDetection::Unknown,
    }
}

#[cfg(any(unix, test))]
fn is_known_unsupported_shell(name: &str) -> bool {
    matches!(
        name,
        "sh" | "dash"
            | "ash"
            | "ksh"
            | "mksh"
            | "pdksh"
            | "yash"
            | "csh"
            | "tcsh"
            | "xonsh"
            | "elvish"
            | "ion"
            | "oil"
            | "osh"
    )
}

#[cfg(any(unix, test))]
fn unsupported_active_shell_message(action: ShellIntegrationAction, shell: &str) -> String {
    format!(
        "error: unsupported active shell '{shell}'\n\n`elio shell {}` {} the active shell.\nsupported shells: {SUPPORTED_SHELLS}\n\n{}",
        action.command(),
        action.active_shell_description(),
        explicit_shell_guidance(action)
    )
}

fn explicit_shell_guidance(action: ShellIntegrationAction) -> String {
    let command = action.command();
    format!(
        "Run one of these explicitly if you want to target another shell:\n  elio shell {command} fish\n  elio shell {command} bash\n  elio shell {command} zsh\n  elio shell {command} nu\n  elio shell {command} pwsh"
    )
}

pub(super) fn shell_name_from_command(command: &str) -> Option<String> {
    let command = command.trim();
    if command.is_empty() {
        return None;
    }

    let first_word = command.split_whitespace().next()?;
    let without_login_prefix = first_word.strip_prefix('-').unwrap_or(first_word);
    let file_name = Path::new(without_login_prefix)
        .file_name()
        .and_then(|name| name.to_str())?;
    Some(file_name.strip_prefix('-').unwrap_or(file_name).to_string())
}
