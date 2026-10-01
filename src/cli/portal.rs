use anyhow::Result;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use elio::portal::terminal::{self, TerminalAdapter};
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use std::env;

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use std::process::Command;

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
const MAX_PARENT_PROCESSES: usize = 6;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PortalCommand {
    Enable,
    Disable,
    Status,
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
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
            // Retry frontend replacement even when routing was already set: a
            // prior invocation can have written it and then failed to refresh.
            elio::reactivate_portal_frontend()?;
            println!("Portal descriptor: {}", result.portal_path.display());
            println!("D-Bus service: {}", result.service_path.display());
            println!("Portal launcher: {}", result.launcher.display());
            println!("FileChooser routing: elio");
            println!("Portal frontend reactivated.");
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
            elio::reactivate_portal_frontend()?;
            println!("Portal frontend reactivated.");
            if restored {
                println!("Restored FileChooser routing.");
            }
        }
        PortalCommand::Status => {
            let status = elio::portal_metadata_status()?;
            let routing = elio::portal_routing_status()?;
            println!("elio FileChooser portal metadata: {}", status.state);
            println!("Portal descriptor: {}", status.portal_path.display());
            println!("D-Bus service: {}", status.service_path.display());
            for line in routing_summary(routing.state) {
                println!("{line}");
            }
            if let Some(path) = routing.effective {
                println!("Portal config: {}", path.display());
            }
            if let Some(value) = routing.value {
                println!("FileChooser: {value}");
            }
        }
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
pub(super) fn execute(_: PortalCommand) -> Result<()> {
    anyhow::bail!("error: `elio portal` is only supported on Linux and FreeBSD")
}

#[cfg(any(test, target_os = "linux", target_os = "freebsd"))]
fn routing_summary(state: elio::PortalRoutingState) -> Vec<String> {
    match state {
        elio::PortalRoutingState::Disabled => vec!["Portal routing: disabled".to_string()],
        elio::PortalRoutingState::RecoveryRequired => {
            vec!["Portal routing: recovery required".to_string()]
        }
        elio::PortalRoutingState::Managed(desktops) => {
            let desktop_specific = desktops.iter().all(|desktop| desktop.desktop_specific);
            let noun = if desktop_specific {
                "desktop"
            } else {
                "configuration"
            };
            let mut lines = vec![format!(
                "Portal routing: enabled for {} {noun}{}",
                desktops.len(),
                if desktops.len() == 1 { "" } else { "s" },
            )];
            lines.extend(desktops.into_iter().map(|desktop| {
                format!(
                    "  • {}{}",
                    desktop.name,
                    if desktop.current { " (current)" } else { "" },
                )
            }));
            lines
        }
    }
}

#[cfg(any(test, target_os = "linux", target_os = "freebsd"))]
fn enable_with<T>(platform: &str, enable: impl FnOnce() -> Result<T>) -> Result<T> {
    if !matches!(platform, "linux" | "freebsd") {
        anyhow::bail!("error: `elio portal enable` is only supported on Linux and FreeBSD")
    }
    enable()
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn detect_current_terminal() -> Option<TerminalAdapter> {
    terminal::detect_with(&real_env_lookup, &parent_commands())
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn real_env_lookup(name: &str) -> Option<String> {
    env::var_os(name).map(|value| value.to_string_lossy().into_owned())
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
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

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
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

#[cfg(any(test, target_os = "linux", target_os = "freebsd"))]
fn parse_parent_process(output: &str) -> Option<(String, String)> {
    let mut fields = output.split_whitespace();
    Some((fields.next()?.to_string(), fields.next()?.to_string()))
}

#[cfg(test)]
#[path = "tests/portal.rs"]
mod tests;
