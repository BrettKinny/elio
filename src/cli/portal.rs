use anyhow::Result;
use elio::portal::terminal::{self, TerminalAdapter};
use std::env;

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

pub(super) fn execute(command: PortalCommand) -> Result<()> {
    match command {
        PortalCommand::Enable => {
            let (configured, terminal) = enable_with(env::consts::OS, || {
                elio::ensure_portal_terminal(detect_current_terminal)
            })?;
            if configured {
                println!("Configured portal terminal: {terminal}");
            } else {
                println!("Portal terminal is already configured: {terminal}");
            }
            let result = elio::enable_portal_metadata()?;
            elio::enable_portal_routing()?;
            println!("Portal descriptor: {}", result.portal_path.display());
            println!("D-Bus service: {}", result.service_path.display());
            println!("Portal launcher: {}", result.launcher.display());
            println!("FileChooser routing: elio");
            if !result.created_portal || !result.created_service {
                println!("Portal metadata was already present and verified.");
            }
        }
        PortalCommand::Disable => {
            let restored = elio::disable_portal_routing()?;
            if elio::portal_routing_references_elio()? {
                println!(
                    "Kept Elio portal metadata: effective FileChooser routing still references elio."
                );
            } else {
                let result = elio::disable_portal_metadata()?;
                if result.removed_portal || result.removed_service {
                    println!("Removed Elio-owned portal metadata.");
                } else {
                    println!("No Elio-owned portal metadata to remove.");
                }
            }
            if restored {
                println!("Restored FileChooser routing.");
            }
        }
        PortalCommand::Status => {
            let status = elio::portal_metadata_status()?;
            let (effective, value, state) = elio::portal_routing_status()?;
            println!("elio FileChooser portal metadata: {}", status.state);
            println!("Portal descriptor: {}", status.portal_path.display());
            println!("D-Bus service: {}", status.service_path.display());
            println!("Portal routing: {state}");
            if let Some(path) = effective {
                println!("Portal config: {}", path.display());
            }
            if let Some(value) = value {
                println!("FileChooser: {value}");
            }
        }
    }
    Ok(())
}

fn enable_with<T>(platform: &str, enable: impl FnOnce() -> Result<T>) -> Result<T> {
    if !matches!(platform, "linux" | "freebsd") {
        anyhow::bail!("error: `elio portal enable` is only supported on Linux and FreeBSD")
    }
    enable()
}

fn detect_current_terminal() -> Option<TerminalAdapter> {
    terminal::detect_with(&real_env_lookup, &parent_commands())
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
