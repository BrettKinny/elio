use super::*;

#[test]
fn accepts_a_replacement_owner_only_after_a_transition() {
    assert!(owner_changed(None, Some(":1.9")));
    assert!(owner_changed(Some(":1.4"), Some(":1.9")));
    assert!(!owner_changed(Some(":1.4"), Some(":1.4")));
    assert!(!owner_changed(None, None));
}

#[test]
fn parses_the_standard_portal_activation_service() {
    let root = temporary_root("service");
    let service = root.join(SERVICE_FILE);
    fs::write(
            &service,
            "[D-BUS Service]\nName=org.freedesktop.portal.Desktop\nExec=/usr/libexec/xdg-desktop-portal --verbose\n",
        )
        .unwrap();
    assert_eq!(
        parse_service(&service).unwrap(),
        Some(CommandSpec {
            program: OsString::from("/usr/libexec/xdg-desktop-portal"),
            args: vec![OsString::from("--verbose")],
            source: Some(service.clone()),
        })
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rejects_a_service_with_a_path_dependent_exec() {
    let root = temporary_root("relative-exec");
    let service = root.join(SERVICE_FILE);
    fs::write(
        &service,
        "[D-BUS Service]\nName=org.freedesktop.portal.Desktop\nExec=xdg-desktop-portal\n",
    )
    .unwrap();
    assert!(
        parse_service(&service)
            .unwrap_err()
            .to_string()
            .contains("non-absolute")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn parses_quoted_service_arguments_without_a_shell() {
    assert_eq!(
        split_exec("/usr/libexec/xdg-desktop-portal --flag='two words'").unwrap(),
        vec![
            OsString::from("/usr/libexec/xdg-desktop-portal"),
            OsString::from("--flag=two words"),
        ]
    );
}

#[test]
fn empty_xdg_data_home_uses_its_default() {
    let default = PathBuf::from("/home/test/.local/share");
    assert_eq!(
        data_home(Some(OsString::new()), || Ok(default.clone())).unwrap(),
        default
    );
}

fn temporary_root(label: &str) -> PathBuf {
    let path = env::temp_dir().join(format!(
        "elio-portal-frontend-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}
