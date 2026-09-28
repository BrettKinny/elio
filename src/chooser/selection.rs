use super::{
    SaveAsState,
    portal::{ExternalCancellation, PortalChooser, PortalChooserMode, external_exit},
};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ChooserExit {
    Confirmed(Vec<PathBuf>),
    Cancelled,
}

#[derive(Default)]
pub(crate) struct ChooserState {
    enabled: bool,
    save_as: Option<SaveAsState>,
    portal: Option<PortalChooser>,
    rejection: Option<&'static str>,
    exit: Option<ChooserExit>,
}

impl ChooserState {
    pub(crate) fn enable(&mut self) {
        self.enabled = true;
    }
    pub(crate) fn enable_save_as(&mut self, name: String) {
        self.enabled = true;
        self.save_as = Some(SaveAsState::new(name));
    }
    #[allow(dead_code)] // Wired by the portal chooser child in the next roadmap step.
    pub(crate) fn enable_portal(&mut self, mode: PortalChooserMode) -> ExternalCancellation {
        let (portal, cancellation) = PortalChooser::new(mode);
        self.enabled = true;
        self.save_as = portal.save_as_state();
        self.portal = Some(portal);
        cancellation
    }
    pub(crate) fn save_as(&self) -> Option<&SaveAsState> {
        self.save_as.as_ref()
    }
    pub(crate) fn save_as_mut(&mut self) -> Option<&mut SaveAsState> {
        self.save_as.as_mut()
    }
    pub(crate) fn is_save_as(&self) -> bool {
        self.save_as.is_some()
    }

    pub(crate) fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn selects_current_directory_when_unmarked(&self) -> bool {
        self.portal
            .as_ref()
            .is_some_and(PortalChooser::selects_current_directory_when_unmarked)
    }

    pub(crate) fn confirm_selection(
        &mut self,
        cwd: &Path,
        focused_path: Option<&Path>,
        selected_paths: Vec<PathBuf>,
    ) -> bool {
        if !self.enabled {
            return false;
        }
        if self.apply_external_cancellation() {
            return true;
        }
        let paths = resolve_selection(cwd, focused_path, selected_paths);
        if let Some(portal) = &self.portal {
            let paths = match portal.constrain_selection(cwd, paths) {
                Ok(paths) => paths,
                Err(message) => {
                    self.rejection = Some(message);
                    return false;
                }
            };
            if !portal.confirm() {
                self.apply_external_cancellation();
                return true;
            }
            self.exit = Some(ChooserExit::Confirmed(paths));
        } else {
            self.exit = Some(ChooserExit::Confirmed(paths));
        }
        true
    }

    pub(crate) fn confirm_path(&mut self, cwd: &Path, path: &Path) -> bool {
        if !self.enabled {
            return false;
        }
        if self.apply_external_cancellation() {
            return true;
        }
        let paths = vec![absolute_path(cwd, path)];
        if let Some(portal) = &self.portal {
            let paths = match portal.constrain_selection(cwd, paths) {
                Ok(paths) => paths,
                Err(message) => {
                    self.rejection = Some(message);
                    return false;
                }
            };
            if !portal.confirm() {
                self.apply_external_cancellation();
                return true;
            }
            self.exit = Some(ChooserExit::Confirmed(paths));
        } else {
            self.exit = Some(ChooserExit::Confirmed(paths));
        }
        true
    }

    pub(crate) fn cancel(&mut self) -> bool {
        if !self.enabled {
            return false;
        }
        self.exit = Some(ChooserExit::Cancelled);
        true
    }

    pub(crate) fn take_exit(&mut self) -> Option<ChooserExit> {
        self.apply_external_cancellation();
        self.exit.take()
    }

    pub(crate) fn apply_external_cancellation(&mut self) -> bool {
        if self.exit.is_none()
            && self
                .portal
                .as_ref()
                .and_then(external_exit)
                .is_some_and(|exit| {
                    self.exit = Some(exit);
                    true
                })
        {
            return true;
        }
        false
    }

    pub(crate) fn exit_is_cancelled(&self) -> bool {
        matches!(self.exit, Some(ChooserExit::Cancelled))
    }

    pub(crate) fn take_rejection(&mut self) -> Option<&'static str> {
        self.rejection.take()
    }

    #[cfg(test)]
    pub(crate) fn exit(&self) -> Option<&ChooserExit> {
        self.exit.as_ref()
    }
}

fn resolve_selection(
    cwd: &Path,
    focused_path: Option<&Path>,
    selected_paths: Vec<PathBuf>,
) -> Vec<PathBuf> {
    if selected_paths.is_empty() {
        return focused_path
            .map(|path| vec![absolute_path(cwd, path)])
            .unwrap_or_default();
    }

    let mut paths = selected_paths
        .iter()
        .map(|path| absolute_path(cwd, path))
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    paths
}

fn absolute_path(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}
