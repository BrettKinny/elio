use super::*;

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
#[test]
fn maps_all_request_selection_kinds_to_the_chooser_contract() {
    for (kind, expected) in [
        (SelectionKind::File, PortalSelectionKind::File),
        (SelectionKind::Directory, PortalSelectionKind::Directory),
        (
            SelectionKind::FileOrDirectory,
            PortalSelectionKind::FileOrDirectory,
        ),
    ] {
        assert_eq!(
            request_mode(ChooserRequestMode::Open {
                kind,
                multiple: true,
            }),
            PortalChooserMode::Open {
                kind: expected,
                multiple: true,
            }
        );
    }
}

#[cfg(not(all(unix, any(target_os = "linux", target_os = "freebsd"))))]
#[test]
fn unsupported_platform_does_not_start_the_chooser_runtime() {
    assert!(run(Path::new("/tmp/request.sock")).is_err());
}

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
#[test]
fn preserves_non_utf8_initial_path_bytes() {
    let path = path_from_bytes(b"/tmp/elio-\xff".to_vec());
    assert_eq!(path_bytes(path), b"/tmp/elio-\xff");
}
