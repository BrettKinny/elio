use crate::portal::terminal::TerminalAdapter;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::{fs, path::Path};

#[derive(Default)]
pub(crate) struct PortalConfig {
    pub(crate) terminal: Option<TerminalAdapter>,
}

#[derive(Deserialize, Default)]
pub(super) struct PortalConfigOverride {
    terminal: Option<TerminalAdapter>,
}

pub(crate) enum ConfigurePortalTerminal {
    Existing(TerminalAdapter),
    Configured(TerminalAdapter),
}

pub(crate) fn ensure_portal_terminal(
    path: &Path,
    detect_terminal: impl FnOnce() -> Option<TerminalAdapter>,
) -> Result<ConfigurePortalTerminal> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => anyhow::bail!(
            "error: failed to read config from {}: {error}",
            path.display()
        ),
    };

    let parsed = super::loading::Config::from_str(&contents)
        .with_context(|| format!("error: failed to parse config from {}", path.display()))?;
    if let Some(terminal) = parsed.portal.terminal {
        return Ok(ConfigurePortalTerminal::Existing(terminal));
    }

    let Some(terminal) = detect_terminal() else {
        anyhow::bail!(
            "error: could not detect a supported terminal; set [portal].terminal manually in {}",
            path.display()
        );
    };

    let updated = insert_terminal(&contents, terminal);
    if let Some(parent) = path.parent()
        && fs::create_dir_all(parent).is_err()
    {
        return Err(manual_config_error(path, terminal));
    }
    if fs::write(path, updated).is_err() {
        return Err(manual_config_error(path, terminal));
    }
    Ok(ConfigurePortalTerminal::Configured(terminal))
}

fn manual_config_error(path: &Path, terminal: TerminalAdapter) -> anyhow::Error {
    anyhow::anyhow!(
        "error: could not update Elio config {}; add this manually:\n\n[portal]\nterminal = \"{}\"",
        path.display(),
        terminal.name()
    )
}

fn insert_terminal(contents: &str, terminal: TerminalAdapter) -> String {
    let assignment = format!("terminal = \"{}\"\n", terminal.name());
    if let Some(portal_start) = table_start(contents, "portal") {
        let section_end = contents[portal_start..]
            .find('\n')
            .map(|offset| portal_start + offset + 1)
            .unwrap_or(contents.len());

        let mut updated = String::with_capacity(contents.len() + assignment.len() + 1);
        updated.push_str(&contents[..section_end]);
        updated.push_str(&assignment);
        updated.push_str(&contents[section_end..]);
        return updated;
    }

    if contents.is_empty() {
        return format!("[portal]\n{assignment}");
    }

    let separator = if contents.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    format!("{contents}{separator}[portal]\n{assignment}")
}

fn table_start(contents: &str, table: &str) -> Option<usize> {
    contents
        .split_inclusive('\n')
        .scan(0, |offset, line| {
            let line_start = *offset;
            *offset += line.len();
            Some((line_start, line))
        })
        .find_map(|(offset, line)| (standard_table_name(line) == Some(table)).then_some(offset))
}

fn standard_table_name(line: &str) -> Option<&str> {
    let line = line
        .split_once('#')
        .map_or(line, |(before, _)| before)
        .trim();
    if line.starts_with("[[") || !line.ends_with(']') {
        return None;
    }
    let name = line.strip_prefix('[')?.strip_suffix(']')?.trim();
    (!name.is_empty()).then_some(name)
}

impl PortalConfig {
    pub(super) fn apply_override(&mut self, overrides: PortalConfigOverride) {
        if let Some(terminal) = overrides.terminal {
            self.terminal = Some(terminal);
        }
    }
}
