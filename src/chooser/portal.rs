use super::{ChooserExit, SaveAsState};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PortalSelectionKind {
    File,
    Directory,
    FileOrDirectory,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PortalChooserMode {
    Open {
        kind: PortalSelectionKind,
        multiple: bool,
    },
    SaveFile {
        initial_name: String,
    },
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ExternalCancellation(Arc<AtomicU8>);

impl ExternalCancellation {
    pub(crate) fn cancel(&self) {
        let _ = self
            .0
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire);
    }

    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire) == 1
    }

    fn confirm(&self) -> bool {
        self.0
            .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
}

#[derive(Debug)]
pub(crate) struct PortalChooser {
    mode: PortalChooserMode,
    cancellation: ExternalCancellation,
}

impl PortalChooser {
    pub(crate) fn new(mode: PortalChooserMode) -> (Self, ExternalCancellation) {
        let cancellation = ExternalCancellation::default();
        let chooser = Self::with_cancellation(mode, cancellation.clone());
        (chooser, cancellation)
    }

    pub(crate) fn with_cancellation(
        mode: PortalChooserMode,
        cancellation: ExternalCancellation,
    ) -> Self {
        Self { mode, cancellation }
    }

    pub(crate) fn save_as_state(&self) -> Option<SaveAsState> {
        match &self.mode {
            PortalChooserMode::Open { .. } => None,
            PortalChooserMode::SaveFile { initial_name } => {
                Some(SaveAsState::new(initial_name.clone()))
            }
        }
    }

    pub(crate) fn constrain_selection(
        &self,
        cwd: &Path,
        paths: Vec<PathBuf>,
    ) -> Result<Vec<PathBuf>, &'static str> {
        let PortalChooserMode::Open { kind, multiple } = self.mode else {
            return Ok(paths);
        };

        let paths = if paths.is_empty() && kind == PortalSelectionKind::Directory {
            vec![cwd.to_path_buf()]
        } else {
            paths
        };
        if paths.is_empty() {
            return Err("Select a path");
        }
        if !multiple && paths.len() != 1 {
            return Err("Select exactly one path");
        }
        for path in &paths {
            let metadata = std::fs::metadata(path).map_err(|_| "Selected path is unavailable")?;
            let valid = match kind {
                PortalSelectionKind::File => metadata.is_file(),
                PortalSelectionKind::Directory => metadata.is_dir(),
                PortalSelectionKind::FileOrDirectory => metadata.is_file() || metadata.is_dir(),
            };
            if !valid {
                return Err(match kind {
                    PortalSelectionKind::File => "Select a file",
                    PortalSelectionKind::Directory => "Select a directory",
                    PortalSelectionKind::FileOrDirectory => "Select a file or directory",
                });
            }
        }
        Ok(paths)
    }

    pub(crate) fn cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }

    pub(crate) fn confirm(&self) -> bool {
        self.cancellation.confirm()
    }

    pub(crate) fn selects_current_directory_when_unmarked(&self) -> bool {
        matches!(
            self.mode,
            PortalChooserMode::Open {
                kind: PortalSelectionKind::Directory,
                ..
            }
        )
    }
}

pub(crate) fn external_exit(chooser: &PortalChooser) -> Option<ChooserExit> {
    chooser.cancelled().then_some(ChooserExit::Cancelled)
}
