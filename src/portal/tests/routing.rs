use super::*;
use std::{
    os::unix::fs::symlink,
    time::{SystemTime, UNIX_EPOCH},
};
fn root(label: &str) -> PathBuf {
    let path = env::temp_dir().join(format!(
        "elio-routing-{label}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}
fn paths(root: &Path, desktops: &[&str]) -> Paths {
    let config_home = root.join("config");
    Paths {
        config_home: config_home.clone(),
        state: root.join("state/elio/portal-routing.json"),
        search: vec![config_home, root.join("etc"), root.join("share")],
        desktops: desktops
            .iter()
            .map(|name| Desktop {
                id: name.to_ascii_lowercase(),
                identifier: (*name).to_string(),
            })
            .collect(),
    }
}
fn config(root: &Path, base: &str, name: &str, text: &str) -> PathBuf {
    let path = root.join(base).join("xdg-desktop-portal").join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, text).unwrap();
    path
}
#[test]
fn xdg_resolution_is_desktop_then_generic_per_location() {
    let root = root("xdg");
    let paths = paths(&root, &["hyprland", "gnome"]);
    let generic = config(
        &root,
        "config",
        "portals.conf",
        "[preferred]\ndefault=gtk\n",
    );
    config(
        &root,
        "etc",
        "hyprland-portals.conf",
        "[preferred]\ndefault=hyprland\n",
    );
    assert_eq!(effective(&paths).unwrap(), Some(generic));
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn system_source_becomes_complete_user_override() {
    let root = root("override");
    let paths = paths(&root, &["hyprland"]);
    let source_path = config(
        &root,
        "etc",
        "hyprland-portals.conf",
        "[preferred]\ndefault=gtk\norg.freedesktop.impl.portal.Settings=gtk\n",
    );
    let target = target(&paths).unwrap();
    assert!(target.created);
    assert_eq!(target.source, Some(source_path));
    let (changed, _) = replace(&source(target.source.as_ref().unwrap()).unwrap(), "elio").unwrap();
    assert!(changed.contains("default=gtk"));
    assert!(changed.contains("Settings=gtk"));
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn stow_link_keeps_logical_link() {
    let root = root("stow");
    let paths = paths(&root, &["hyprland"]);
    let destination = root.join("dotfiles/config");
    fs::create_dir_all(destination.parent().unwrap()).unwrap();
    fs::write(&destination, "[preferred]\ndefault=gtk\n").unwrap();
    let logical = paths
        .config_home
        .join("xdg-desktop-portal/hyprland-portals.conf");
    fs::create_dir_all(logical.parent().unwrap()).unwrap();
    symlink(&destination, &logical).unwrap();
    let target = target(&paths).unwrap();
    assert_eq!(target.target, destination);
    assert!(
        fs::symlink_metadata(logical)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn readonly_user_link_is_not_replaced() {
    let root = root("readonly");
    let paths = paths(&root, &["hyprland"]);
    let destination = root.join("store/config");
    fs::create_dir_all(destination.parent().unwrap()).unwrap();
    fs::write(&destination, "[preferred]\ndefault=gtk\n").unwrap();
    fs::set_permissions(&destination, fs::Permissions::from_mode(0o444)).unwrap();
    let logical = paths
        .config_home
        .join("xdg-desktop-portal/hyprland-portals.conf");
    fs::create_dir_all(logical.parent().unwrap()).unwrap();
    symlink(&destination, &logical).unwrap();
    assert!(
        target(&paths)
            .unwrap_err()
            .to_string()
            .contains("declarative/read-only")
    );
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn preserves_comments_and_restores_absent_key() {
    let before = "[preferred] # keep\ndefault = gtk # keep\n[other]\nkey=value\n";
    let (enabled, previous) = replace(before, "elio").unwrap();
    assert_eq!(previous, None);
    assert!(enabled.contains("default = gtk # keep"));
    assert_eq!(restore(&enabled, None).unwrap(), before);
}
#[test]
fn enable_then_disable_restores_only_filechooser_after_unrelated_drift() {
    let root = root("roundtrip");
    let paths = paths(&root, &["hyprland"]);
    let file = config(
        &root,
        "config",
        "hyprland-portals.conf",
        "[preferred]\ndefault=gtk\norg.freedesktop.impl.portal.FileChooser=gtk\n",
    );
    enable_at(&paths).unwrap();
    assert!(source(&file).unwrap().contains("FileChooser=elio"));
    fs::write(&file, format!("{}# user change\n", source(&file).unwrap())).unwrap();
    assert!(disable_at(&paths).unwrap());
    let restored = source(&file).unwrap();
    assert!(restored.contains("FileChooser=gtk"));
    assert!(restored.contains("# user change"));
    assert!(state(&paths.state).unwrap().is_none());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn disable_never_overwrites_a_later_filechooser_choice() {
    let root = root("superseded");
    let paths = paths(&root, &["hyprland"]);
    let file = config(
        &root,
        "config",
        "hyprland-portals.conf",
        "[preferred]\norg.freedesktop.impl.portal.FileChooser=gtk\n",
    );
    enable_at(&paths).unwrap();
    fs::write(
        &file,
        "[preferred]\norg.freedesktop.impl.portal.FileChooser=kde\n",
    )
    .unwrap();
    assert!(!disable_at(&paths).unwrap());
    assert!(source(&file).unwrap().contains("FileChooser=kde"));
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn unchanged_created_override_is_removed_on_disable() {
    let root = root("created");
    let paths = paths(&root, &["hyprland"]);
    config(
        &root,
        "etc",
        "hyprland-portals.conf",
        "[preferred]\ndefault=gtk\n",
    );
    let override_path = paths
        .config_home
        .join("xdg-desktop-portal/hyprland-portals.conf");
    enable_at(&paths).unwrap();
    assert!(override_path.exists());
    assert!(disable_at(&paths).unwrap());
    assert!(!override_path.exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn enabling_hyprland_does_not_change_gnome_routing() {
    let root = root("hyprland-only");
    let gnome = paths(&root, &["gnome"]);
    let hyprland = paths(&root, &["hyprland"]);
    let gnome_before = "[preferred]\ndefault=gtk\norg.freedesktop.impl.portal.FileChooser=gtk\n";
    let gnome_file = config(&root, "config", "gnome-portals.conf", gnome_before);
    let hyprland_file = config(
        &root,
        "config",
        "hyprland-portals.conf",
        "[preferred]\ndefault=hyprland\norg.freedesktop.impl.portal.FileChooser=gtk\n",
    );

    enable_at(&hyprland).unwrap();

    assert_eq!(source(&gnome_file).unwrap(), gnome_before);
    assert!(source(&hyprland_file).unwrap().contains("FileChooser=elio"));
    assert_eq!(state(&gnome.state).unwrap().unwrap().records.len(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn enabling_gnome_does_not_change_hyprland_routing() {
    let root = root("gnome-only");
    let gnome = paths(&root, &["gnome"]);
    let hyprland = paths(&root, &["hyprland"]);
    let hyprland_before =
        "[preferred]\ndefault=hyprland\norg.freedesktop.impl.portal.FileChooser=gtk\n";
    let gnome_file = config(
        &root,
        "config",
        "gnome-portals.conf",
        "[preferred]\ndefault=gtk\norg.freedesktop.impl.portal.FileChooser=gtk\n",
    );
    let hyprland_file = config(&root, "config", "hyprland-portals.conf", hyprland_before);

    enable_at(&gnome).unwrap();

    assert!(source(&gnome_file).unwrap().contains("FileChooser=elio"));
    assert_eq!(source(&hyprland_file).unwrap(), hyprland_before);
    assert_eq!(state(&hyprland.state).unwrap().unwrap().records.len(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn enabling_multiple_desktops_keeps_each_owned_route() {
    let root = root("multiple-enable");
    let gnome = paths(&root, &["gnome"]);
    let hyprland = paths(&root, &["hyprland"]);
    let gnome_file = config(
        &root,
        "config",
        "gnome-portals.conf",
        "[preferred]\ndefault=gtk\norg.freedesktop.impl.portal.FileChooser=gtk\n",
    );
    let hyprland_file = config(
        &root,
        "config",
        "hyprland-portals.conf",
        "[preferred]\ndefault=hyprland\norg.freedesktop.impl.portal.FileChooser=gtk\n",
    );

    enable_at(&gnome).unwrap();
    let enabled_gnome = source(&gnome_file).unwrap();
    let gnome_record = state(&gnome.state).unwrap().unwrap().records.remove(0);
    enable_at(&hyprland).unwrap();

    let state = state(&gnome.state).unwrap().unwrap();
    assert_eq!(source(&gnome_file).unwrap(), enabled_gnome);
    assert!(source(&hyprland_file).unwrap().contains("FileChooser=elio"));
    assert_eq!(state.records.len(), 2);
    let saved_gnome = state
        .records
        .iter()
        .find(|record| record.logical == gnome_record.logical)
        .unwrap();
    assert_eq!(saved_gnome.target, gnome_record.target);
    assert_eq!(saved_gnome.before, gnome_record.before);
    assert_eq!(saved_gnome.after, gnome_record.after);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn disabling_multiple_desktops_restores_each_original_route() {
    let root = root("multiple-disable");
    let gnome = paths(&root, &["gnome"]);
    let hyprland = paths(&root, &["hyprland"]);
    let gnome_before = "[preferred]\ndefault=gtk\norg.freedesktop.impl.portal.FileChooser=gtk\n";
    let hyprland_before =
        "[preferred]\ndefault=hyprland\norg.freedesktop.impl.portal.FileChooser=gtk\n";
    let gnome_file = config(&root, "config", "gnome-portals.conf", gnome_before);
    let hyprland_file = config(&root, "config", "hyprland-portals.conf", hyprland_before);

    enable_at(&gnome).unwrap();
    enable_at(&hyprland).unwrap();
    assert!(disable_at(&hyprland).unwrap());

    assert_eq!(source(&gnome_file).unwrap(), gnome_before);
    assert_eq!(source(&hyprland_file).unwrap(), hyprland_before);
    assert!(state(&gnome.state).unwrap().is_none());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn disabling_multiple_desktops_preserves_a_user_modified_route() {
    let root = root("multiple-ownership");
    let gnome = paths(&root, &["gnome"]);
    let hyprland = paths(&root, &["hyprland"]);
    let hyprland_before =
        "[preferred]\ndefault=hyprland\norg.freedesktop.impl.portal.FileChooser=gtk\n";
    let gnome_file = config(
        &root,
        "config",
        "gnome-portals.conf",
        "[preferred]\ndefault=gtk\norg.freedesktop.impl.portal.FileChooser=gtk\n",
    );
    let hyprland_file = config(&root, "config", "hyprland-portals.conf", hyprland_before);

    enable_at(&gnome).unwrap();
    enable_at(&hyprland).unwrap();
    let user_choice = "[preferred]\ndefault=gtk\norg.freedesktop.impl.portal.FileChooser=kde\n";
    fs::write(&gnome_file, user_choice).unwrap();

    assert!(disable_at(&hyprland).unwrap());

    assert_eq!(source(&gnome_file).unwrap(), user_choice);
    assert_eq!(source(&hyprland_file).unwrap(), hyprland_before);
    assert!(state(&gnome.state).unwrap().is_none());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn prepared_created_target_missing_is_discarded() {
    let root = root("prepared-missing");
    let paths = paths(&root, &["hyprland"]);
    let logical = config(
        &root,
        "etc",
        "hyprland-portals.conf",
        "[preferred]\ndefault=gtk\n",
    );
    let target = paths
        .config_home
        .join("xdg-desktop-portal/hyprland-portals.conf");
    write_state(
        &paths.state,
        &State {
            records: vec![Record {
                phase: Phase::Prepared,
                logical,
                target,
                desktop: Some("hyprland".to_string()),
                created: true,
                previous: None,
                before: hash(""),
                after: hash(
                    "[preferred]\ndefault=gtk\norg.freedesktop.impl.portal.FileChooser=elio\n",
                ),
            }],
        },
    )
    .unwrap();
    reconcile(&paths).unwrap();
    assert!(state(&paths.state).unwrap().is_none());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn desktop_specific_records_require_a_matching_desktop_identifier() {
    let root = root("missing-desktop-identifier");
    let paths = paths(&root, &["GNOME"]);
    let logical = root.join("config/xdg-desktop-portal/gnome-portals.conf");
    let routing_state = State {
        records: vec![Record {
            phase: Phase::Committed,
            logical: logical.clone(),
            target: logical,
            desktop: None,
            created: false,
            previous: Some("gtk".to_string()),
            before: hash("before"),
            after: hash("after"),
        }],
    };
    fs::create_dir_all(paths.state.parent().unwrap()).unwrap();
    let mut stored = serde_json::to_value(&routing_state).unwrap();
    stored["records"][0]
        .as_object_mut()
        .unwrap()
        .remove("desktop");
    fs::write(&paths.state, serde_json::to_vec(&stored).unwrap()).unwrap();

    assert!(
        state(&paths.state)
            .unwrap_err()
            .to_string()
            .contains("missing its matching desktop identifier")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn checked_mutations_refuse_stale_or_new_targets() {
    let root = root("recheck");
    let existing = root.join("existing.conf");
    fs::write(&existing, "before").unwrap();
    let expected = hash("before");
    fs::write(&existing, "user change").unwrap();
    assert!(atomic_checked(&existing, b"elio", MODE, Expected::Hash(&expected),).is_err());
    assert_eq!(source(&existing).unwrap(), "user change");

    let new = root.join("new.conf");
    fs::write(&new, "user config").unwrap();
    assert!(atomic_checked(&new, b"elio", MODE, Expected::Missing).is_err());
    assert_eq!(source(&new).unwrap(), "user config");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn semicolon_values_are_preserved_for_metadata_decision() {
    assert_eq!(
        value("[preferred]\norg.freedesktop.impl.portal.FileChooser=gtk;elio\n").unwrap(),
        Some("gtk;elio".to_string())
    );
    assert_eq!(
        default("[preferred]\ndefault=gtk;elio\n").unwrap(),
        Some("gtk;elio".to_string())
    );
}

fn managed(status: Status) -> Vec<ManagedDesktop> {
    match status.state {
        StatusState::Managed(desktops) => desktops,
        _ => panic!("expected managed portal routing"),
    }
}

#[test]
fn status_lists_one_managed_desktop_and_marks_it_current() {
    let root = root("status-one");
    let paths = paths(&root, &["Hyprland"]);
    config(
        &root,
        "config",
        "hyprland-portals.conf",
        "[preferred]\norg.freedesktop.impl.portal.FileChooser=gtk\n",
    );

    enable_at(&paths).unwrap();
    let desktops = managed(status_at(&paths).unwrap());

    assert_eq!(desktops.len(), 1);
    assert_eq!(desktops[0].name, "Hyprland");
    assert!(desktops[0].current);
    assert!(desktops[0].desktop_specific);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn status_lists_multiple_managed_desktops() {
    let root = root("status-multiple");
    let gnome = paths(&root, &["GNOME"]);
    let hyprland = paths(&root, &["Hyprland"]);
    config(
        &root,
        "config",
        "gnome-portals.conf",
        "[preferred]\norg.freedesktop.impl.portal.FileChooser=gtk\n",
    );
    config(
        &root,
        "config",
        "hyprland-portals.conf",
        "[preferred]\norg.freedesktop.impl.portal.FileChooser=gtk\n",
    );

    enable_at(&gnome).unwrap();
    enable_at(&hyprland).unwrap();
    let desktops = managed(status_at(&hyprland).unwrap());

    assert_eq!(desktops.len(), 2);
    assert_eq!(desktops[0].name, "GNOME");
    assert!(!desktops[0].current);
    assert_eq!(desktops[1].name, "Hyprland");
    assert!(desktops[1].current);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn status_does_not_mark_a_managed_desktop_current_when_another_is_active() {
    let root = root("status-current-unmanaged");
    let hyprland = paths(&root, &["Hyprland"]);
    let gnome = paths(&root, &["GNOME"]);
    config(
        &root,
        "config",
        "hyprland-portals.conf",
        "[preferred]\norg.freedesktop.impl.portal.FileChooser=gtk\n",
    );
    config(
        &root,
        "config",
        "gnome-portals.conf",
        "[preferred]\norg.freedesktop.impl.portal.FileChooser=gtk\n",
    );

    enable_at(&hyprland).unwrap();
    let status = status_at(&gnome).unwrap();
    let desktops = managed(status);

    assert_eq!(desktops.len(), 1);
    assert_eq!(desktops[0].name, "Hyprland");
    assert!(!desktops[0].current);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn status_uses_the_matching_identifier_from_a_multi_desktop_session() {
    let root = root("status-multi-value");
    let paths = paths(&root, &["Budgie", "GNOME"]);
    config(
        &root,
        "config",
        "budgie-portals.conf",
        "[preferred]\norg.freedesktop.impl.portal.FileChooser=gtk\n",
    );

    enable_at(&paths).unwrap();
    let desktops = managed(status_at(&paths).unwrap());

    assert_eq!(desktops.len(), 1);
    assert_eq!(desktops[0].name, "Budgie");
    assert!(desktops[0].current);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn status_reports_a_managed_generic_portals_config() {
    let root = root("status-generic");
    let paths = paths(&root, &[]);
    config(
        &root,
        "config",
        "portals.conf",
        "[preferred]\norg.freedesktop.impl.portal.FileChooser=gtk\n",
    );

    enable_at(&paths).unwrap();
    let desktops = managed(status_at(&paths).unwrap());

    assert_eq!(desktops.len(), 1);
    assert_eq!(desktops[0].name, "Default portal configuration");
    assert!(desktops[0].current);
    assert!(!desktops[0].desktop_specific);
    fs::remove_dir_all(root).unwrap();
}
