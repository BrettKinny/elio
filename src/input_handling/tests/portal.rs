use super::super::*;
use super::helpers::{cleanup_app_temp_root, temp_path, wait_for_directory_load};
use crate::chooser::{
    ChooserExit,
    portal::{PortalChooserMode, PortalSelectionKind},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::fs;

fn key(app: &mut App, code: KeyCode) {
    app.handle_save_as_key(KeyEvent::new(code, KeyModifiers::NONE));
}

#[test]
fn portal_save_file_submits_through_existing_save_as_flow() {
    let root = temp_path("portal-save-file");
    let mut app = App::new_at(root.clone()).unwrap();
    app.enable_portal_chooser_mode(PortalChooserMode::SaveFile {
        initial_name: "report.txt".into(),
    });
    assert_eq!(app.status, "Save as mode");

    app.confirm_chooser();
    key(&mut app, KeyCode::Enter);

    assert_eq!(
        app.take_chooser_exit(),
        Some(ChooserExit::Confirmed(vec![root.join("report.txt")]))
    );
    cleanup_app_temp_root(app, root);
}

#[test]
fn portal_open_mode_uses_chooser_status_with_external_cancellation() {
    let root = temp_path("portal-open-status");
    let mut app = App::new_at(root.clone()).unwrap();
    app.enable_portal_chooser_mode_with_cancellation(
        PortalChooserMode::Open {
            kind: PortalSelectionKind::File,
            multiple: false,
        },
        Default::default(),
    );

    assert_eq!(app.status, "Chooser mode");
    cleanup_app_temp_root(app, root);
}

#[test]
fn rejected_portal_selection_is_shown_in_status() {
    let root = temp_path("portal-rejection");
    let directory = root.join("directory");
    fs::create_dir(&directory).unwrap();
    let mut app = App::new_at(root.clone()).unwrap();
    wait_for_directory_load(&mut app);
    let directory_index = app
        .file_browser
        .entries
        .iter()
        .position(|entry| entry.path == directory)
        .unwrap();
    app.select_index(directory_index);
    app.enable_portal_chooser_mode(PortalChooserMode::Open {
        kind: PortalSelectionKind::File,
        multiple: false,
    });

    app.confirm_chooser();

    assert_eq!(app.status, "Select a file");
    assert!(!app.should_quit);
    cleanup_app_temp_root(app, root);
}

#[test]
fn portal_directory_selection_confirms_the_focused_directory() {
    let root = temp_path("portal-directory");
    let directory = root.join("directory");
    fs::create_dir(&directory).unwrap();
    let mut app = App::new_at(root.clone()).unwrap();
    wait_for_directory_load(&mut app);
    let directory_index = app
        .file_browser
        .entries
        .iter()
        .position(|entry| entry.path == directory)
        .unwrap();
    app.select_index(directory_index);
    app.enable_portal_chooser_mode(PortalChooserMode::Open {
        kind: PortalSelectionKind::Directory,
        multiple: false,
    });

    app.confirm_chooser();

    assert_eq!(
        app.take_chooser_exit(),
        Some(ChooserExit::Confirmed(vec![directory]))
    );
    cleanup_app_temp_root(app, root);
}

#[test]
fn portal_directory_selection_chooses_current_directory_when_focused_on_a_file() {
    let root = temp_path("portal-directory-current");
    let file = root.join("file");
    fs::write(&file, "file").unwrap();
    let mut app = App::new_at(root.clone()).unwrap();
    wait_for_directory_load(&mut app);
    let file_index = app
        .file_browser
        .entries
        .iter()
        .position(|entry| entry.path == file)
        .unwrap();
    app.select_index(file_index);
    app.enable_portal_chooser_mode(PortalChooserMode::Open {
        kind: PortalSelectionKind::Directory,
        multiple: false,
    });

    app.confirm_chooser();

    assert_eq!(
        app.take_chooser_exit(),
        Some(ChooserExit::Confirmed(vec![root.clone()]))
    );
    cleanup_app_temp_root(app, root);
}
