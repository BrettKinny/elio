use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum PortalTerminal {
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

#[derive(Default)]
pub(crate) struct PortalConfig {
    pub(crate) terminal: Option<PortalTerminal>,
}

#[derive(Deserialize, Default)]
pub(super) struct PortalConfigOverride {
    terminal: Option<PortalTerminal>,
}

impl PortalConfig {
    pub(super) fn apply_override(&mut self, overrides: PortalConfigOverride) {
        if let Some(terminal) = overrides.terminal {
            self.terminal = Some(terminal);
        }
    }
}
