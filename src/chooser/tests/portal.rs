use super::super::{
    ChooserExit, ChooserState,
    portal::{PortalChooserMode, PortalSelectionKind},
};
use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

fn temp(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "elio-portal-chooser-{label}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[test]
fn portal_open_enforces_kind_and_cardinality() {
    let root = temp("open");
    let file = root.join("file");
    let second_file = root.join("second-file");
    let directory = root.join("directory");
    fs::create_dir_all(&directory).unwrap();
    fs::write(&file, "file").unwrap();
    fs::write(&second_file, "file").unwrap();

    let mut chooser = ChooserState::default();
    chooser.enable_portal(PortalChooserMode::Open {
        kind: PortalSelectionKind::File,
        multiple: false,
    });
    assert!(!chooser.confirm_selection(&root, None, vec![directory.clone()]));
    assert!(!chooser.confirm_path(&root, &directory));
    assert!(!chooser.confirm_selection(&root, None, vec![file.clone(), second_file]));
    assert!(chooser.confirm_selection(&root, None, vec![file.clone()]));
    assert_eq!(chooser.exit(), Some(&ChooserExit::Confirmed(vec![file])));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn portal_directory_selection_can_select_current_directory() {
    let root = temp("directory");
    fs::create_dir_all(&root).unwrap();

    let mut chooser = ChooserState::default();
    chooser.enable_portal(PortalChooserMode::Open {
        kind: PortalSelectionKind::Directory,
        multiple: false,
    });
    assert!(chooser.confirm_selection(&root, None, Vec::new()));
    assert_eq!(
        chooser.exit(),
        Some(&ChooserExit::Confirmed(vec![root.clone()]))
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn portal_open_accepts_files_and_directories_when_requested() {
    let root = temp("either");
    let file = root.join("file");
    let directory = root.join("directory");
    fs::create_dir_all(&directory).unwrap();
    fs::write(&file, "file").unwrap();

    let mut chooser = ChooserState::default();
    chooser.enable_portal(PortalChooserMode::Open {
        kind: PortalSelectionKind::FileOrDirectory,
        multiple: true,
    });
    assert!(chooser.confirm_selection(&root, None, vec![directory.clone(), file.clone()]));
    assert_eq!(
        chooser.exit(),
        Some(&ChooserExit::Confirmed(vec![directory, file]))
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn portal_save_file_reuses_save_as_state() {
    let mut chooser = ChooserState::default();
    chooser.enable_portal(PortalChooserMode::SaveFile {
        initial_name: "report.txt".into(),
    });

    assert!(chooser.is_save_as());
    assert_eq!(chooser.save_as().unwrap().input(), "report.txt");
}

#[test]
fn portal_external_cancellation_wins_over_confirmation() {
    let mut chooser = ChooserState::default();
    let cancellation = chooser.enable_portal(PortalChooserMode::Open {
        kind: PortalSelectionKind::File,
        multiple: false,
    });

    cancellation.cancel();
    assert!(chooser.confirm_path(std::path::Path::new("/"), std::path::Path::new("missing")));
    assert_eq!(chooser.exit(), Some(&ChooserExit::Cancelled));
}

#[test]
fn portal_external_cancellation_produces_cancelled_exit() {
    let mut chooser = ChooserState::default();
    let cancellation = chooser.enable_portal(PortalChooserMode::Open {
        kind: PortalSelectionKind::File,
        multiple: false,
    });

    cancellation.cancel();
    assert!(chooser.apply_external_cancellation());
    assert_eq!(chooser.exit(), Some(&ChooserExit::Cancelled));
}
