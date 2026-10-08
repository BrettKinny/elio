use serde::Deserialize;
use std::collections::BTreeMap;

/// Command used by the `play_audio` action to open a TUI audio browser
/// (default: [tap](https://github.com/timdubbins/tap)) rooted at the selected
/// file or folder. The command may contain a single `{path}` placeholder; the
/// selected item's path is substituted at launch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AudioPreviewConfig {
    pub command: String,
}

#[derive(Deserialize, Default)]
pub(super) struct AudioPreviewConfigOverride {
    command: Option<String>,
    #[serde(flatten)]
    unknown: BTreeMap<String, toml::Value>,
}

impl AudioPreviewConfig {
    pub(crate) const DEFAULT_COMMAND: &'static str = "tap {path}";

    pub(super) fn from_override(overrides: AudioPreviewConfigOverride, defaults: &Self) -> Self {
        for key in overrides.unknown.keys() {
            eprintln!("elio: audio_preview.{key}: unknown audio preview config key; ignoring");
        }

        let mut resolved = defaults.clone();
        if let Some(command) = overrides.command {
            let command = command.trim();
            if command.is_empty() {
                eprintln!(
                    "elio: audio_preview.command: must be a non-empty string; using default '{default}'",
                    default = Self::DEFAULT_COMMAND
                );
            } else {
                resolved.command = command.to_string();
            }
        }
        resolved
    }

    /// The configured command with `{path}` replaced by the selected item.
    pub(crate) fn command_for_path(&self, path: &std::path::Path) -> String {
        self.command
            .replace("{path}", &path.to_string_lossy())
    }

    /// The first token of the configured command (used for availability checks).
    pub(crate) fn program_name(&self) -> Option<&str> {
        self.command.split_whitespace().next()
    }
}

impl Default for AudioPreviewConfig {
    fn default() -> Self {
        Self {
            command: Self::DEFAULT_COMMAND.to_string(),
        }
    }
}
