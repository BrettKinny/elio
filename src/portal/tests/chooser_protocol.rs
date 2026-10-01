use super::*;
#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
use std::{sync::mpsc, thread};

#[test]
fn error_code_rejects_unbounded_or_non_ascii_values() {
    assert!(matches!(
        ErrorCode::new("x".repeat(MAX_ERROR_CODE_BYTES + 1)),
        Err(ProtocolError::InvalidErrorCode)
    ));
    assert!(matches!(
        ErrorCode::new("café"),
        Err(ProtocolError::InvalidErrorCode)
    ));
}

#[cfg(not(all(unix, any(target_os = "linux", target_os = "freebsd"))))]
#[test]
fn endpoints_fail_closed_on_unsupported_platforms() {
    assert!(matches!(
        ServiceEndpoint::bind(),
        Err(ProtocolError::UnsupportedPlatform)
    ));
}

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
#[test]
fn request_ready_and_single_result_round_trip_raw_path_bytes() {
    let endpoint = ServiceEndpoint::bind().unwrap();
    let socket = endpoint.socket_path().to_path_buf();
    let (sent, received) = mpsc::channel();
    let child = thread::spawn(move || {
        let mut child =
            ChildEndpoint::connect_until(&socket, Instant::now() + Duration::from_secs(1)).unwrap();
        let request = child.receive_request().unwrap();
        sent.send(request).unwrap();
        child.send_ready().unwrap();
        child
            .send_result(Message::Accepted {
                paths: vec![b"/tmp/not-utf8-\xff".to_vec()],
            })
            .unwrap();
    });
    let mut service = endpoint
        .accept_until(Instant::now() + Duration::from_secs(1))
        .unwrap();
    let request = ChooserRequest {
        mode: ChooserRequestMode::Open {
            kind: SelectionKind::File,
            multiple: false,
        },
        initial_path: Some(b"/tmp/initial-\xff".to_vec()),
    };
    service.send_request(&request).unwrap();
    assert_eq!(received.recv().unwrap(), request);
    service.receive_ready().unwrap();
    assert_eq!(
        service.receive_result().unwrap(),
        Message::Accepted {
            paths: vec![b"/tmp/not-utf8-\xff".to_vec()]
        }
    );
    child.join().unwrap();
}

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
#[test]
fn child_cannot_send_two_terminal_results() {
    let endpoint = ServiceEndpoint::bind().unwrap();
    let socket = endpoint.socket_path().to_path_buf();
    let child = thread::spawn(move || {
        let mut child =
            ChildEndpoint::connect_until(&socket, Instant::now() + Duration::from_secs(1)).unwrap();
        child.receive_request().unwrap();
        child.send_ready().unwrap();
        child.send_result(Message::Cancelled).unwrap();
        child.send_result(Message::Cancelled)
    });
    let mut service = endpoint
        .accept_until(Instant::now() + Duration::from_secs(1))
        .unwrap();
    service
        .send_request(&ChooserRequest {
            mode: ChooserRequestMode::SaveFile {
                initial_name: "name".into(),
            },
            initial_path: None,
        })
        .unwrap();
    service.receive_ready().unwrap();
    assert_eq!(service.receive_result().unwrap(), Message::Cancelled);
    assert!(matches!(
        child.join().unwrap(),
        Err(ProtocolError::InvalidState(_))
    ));
}

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
#[test]
fn connection_deadline_expires_without_child() {
    let endpoint = ServiceEndpoint::bind().unwrap();
    assert!(matches!(
        endpoint.accept_until(Instant::now() + Duration::from_millis(15)),
        Err(ProtocolError::Timeout)
    ));
}

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
#[test]
fn ready_handshake_timeout_does_not_block_the_service() {
    let endpoint = ServiceEndpoint::bind().unwrap();
    let socket = endpoint.socket_path().to_path_buf();
    let child = thread::spawn(move || {
        let _child =
            ChildEndpoint::connect_until(&socket, Instant::now() + Duration::from_secs(1)).unwrap();
        thread::sleep(Duration::from_millis(100));
    });
    let mut service = endpoint
        .accept_until(Instant::now() + Duration::from_secs(1))
        .unwrap();
    service
        .set_timeout(Some(Duration::from_millis(15)))
        .unwrap();
    service
        .send_request(&ChooserRequest {
            mode: ChooserRequestMode::Open {
                kind: SelectionKind::File,
                multiple: false,
            },
            initial_path: None,
        })
        .unwrap();
    assert!(matches!(service.receive_ready(), Err(ProtocolError::Io(_))));
    child.join().unwrap();
}

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
#[test]
fn service_rejects_accepted_results_with_wrong_cardinality() {
    let request = ChooserRequest {
        mode: ChooserRequestMode::Open {
            kind: SelectionKind::File,
            multiple: false,
        },
        initial_path: None,
    };
    for paths in [Vec::new(), vec![b"one".to_vec(), b"two".to_vec()]] {
        let result = validate_result(&request, &Message::Accepted { paths });
        assert!(result.is_err());
    }
    assert!(
        validate_result(
            &request,
            &Message::Accepted {
                paths: vec![b"one".to_vec()],
            },
        )
        .is_ok()
    );
}

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
#[test]
fn cancellation_handle_writes_while_service_waits_for_result() {
    let endpoint = ServiceEndpoint::bind().unwrap();
    let socket = endpoint.socket_path().to_path_buf();
    let child = thread::spawn(move || {
        let mut child =
            ChildEndpoint::connect_until(&socket, Instant::now() + Duration::from_secs(1)).unwrap();
        child.receive_request().unwrap();
        child.send_ready().unwrap();
        child.receive_cancel().unwrap();
        child.send_result(Message::Cancelled).unwrap();
    });
    let mut service = endpoint
        .accept_until(Instant::now() + Duration::from_secs(1))
        .unwrap();
    service
        .send_request(&ChooserRequest {
            mode: ChooserRequestMode::Open {
                kind: SelectionKind::File,
                multiple: false,
            },
            initial_path: None,
        })
        .unwrap();
    service.receive_ready().unwrap();
    let mut cancellation = service.cancellation_handle().unwrap();
    let result = thread::spawn(move || service.receive_result());
    cancellation.send_cancel().unwrap();
    assert_eq!(result.join().unwrap().unwrap(), Message::Cancelled);
    child.join().unwrap();
}

#[test]
fn frame_cap_allows_large_multi_selection_payloads() {
    let payload = serde_json::to_vec(&Message::Accepted {
        paths: vec![vec![255; 4096]; 128],
    })
    .unwrap();
    assert!(payload.len() > 64 * 1024);
    assert!(payload.len() <= MAX_MESSAGE_BYTES);
}

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
#[test]
fn service_disconnect_cancels_the_chooser_contract() {
    use crate::chooser::{
        ChooserState,
        portal::{ExternalCancellation, PortalChooserMode, PortalSelectionKind},
    };

    let endpoint = ServiceEndpoint::bind().unwrap();
    let socket = endpoint.socket_path().to_path_buf();
    let cancellation = ExternalCancellation::default();
    let child_cancellation = cancellation.clone();
    let child = thread::spawn(move || {
        let mut child =
            ChildEndpoint::connect_until(&socket, Instant::now() + Duration::from_secs(1)).unwrap();
        child.receive_request().unwrap();
        child.send_ready().unwrap();
        child.watch_for_cancel(child_cancellation).unwrap();
    });
    let mut service = endpoint
        .accept_until(Instant::now() + Duration::from_secs(1))
        .unwrap();
    service
        .send_request(&ChooserRequest {
            mode: ChooserRequestMode::Open {
                kind: SelectionKind::File,
                multiple: false,
            },
            initial_path: None,
        })
        .unwrap();
    service.receive_ready().unwrap();
    drop(service);

    let mut chooser = ChooserState::default();
    chooser.enable_portal_with_cancellation(
        PortalChooserMode::Open {
            kind: PortalSelectionKind::File,
            multiple: false,
        },
        cancellation,
    );
    let deadline = Instant::now() + Duration::from_secs(1);
    while !chooser.apply_external_cancellation() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    assert!(chooser.exit_is_cancelled());
    child.join().unwrap();
}

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
#[test]
fn cancellation_frame_reaches_the_chooser_contract() {
    use crate::chooser::{
        ChooserState,
        portal::{ExternalCancellation, PortalChooserMode, PortalSelectionKind},
    };

    let endpoint = ServiceEndpoint::bind().unwrap();
    let socket = endpoint.socket_path().to_path_buf();
    let cancellation = ExternalCancellation::default();
    let child_cancellation = cancellation.clone();
    let child = thread::spawn(move || {
        let mut child =
            ChildEndpoint::connect_until(&socket, Instant::now() + Duration::from_secs(1)).unwrap();
        child.receive_request().unwrap();
        child.send_ready().unwrap();
        child.watch_for_cancel(child_cancellation).unwrap();
    });
    let mut service = endpoint
        .accept_until(Instant::now() + Duration::from_secs(1))
        .unwrap();
    service
        .send_request(&ChooserRequest {
            mode: ChooserRequestMode::Open {
                kind: SelectionKind::File,
                multiple: false,
            },
            initial_path: None,
        })
        .unwrap();
    service.receive_ready().unwrap();
    service.send_cancel().unwrap();

    let mut chooser = ChooserState::default();
    chooser.enable_portal_with_cancellation(
        PortalChooserMode::Open {
            kind: PortalSelectionKind::File,
            multiple: false,
        },
        cancellation,
    );
    let deadline = Instant::now() + Duration::from_secs(1);
    while !chooser.apply_external_cancellation() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    assert!(chooser.exit_is_cancelled());
    child.join().unwrap();
}

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
#[test]
fn malformed_frames_are_rejected_before_payload_allocation() {
    let cases = [
        (b"NOPE".as_slice(), VERSION, 0u32, Vec::new()),
        (MAGIC.as_slice(), VERSION + 1, 0, Vec::new()),
        (
            MAGIC.as_slice(),
            VERSION,
            (MAX_MESSAGE_BYTES + 1) as u32,
            Vec::new(),
        ),
        (MAGIC.as_slice(), VERSION, 1, b"x".to_vec()),
    ];
    for (magic, version, length, payload) in cases {
        let (mut writer, mut reader) = UnixStream::pair().unwrap();
        writer.write_all(magic).unwrap();
        writer.write_all(&version.to_be_bytes()).unwrap();
        writer.write_all(&length.to_be_bytes()).unwrap();
        writer.write_all(&payload).unwrap();
        assert!(read_message(&mut reader).is_err());
    }
}

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
#[test]
fn rejects_symlinked_intermediate_runtime_directory() {
    use std::os::unix::fs::symlink;

    let base = std::env::temp_dir().join(format!(
        "elio-portal-root-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    let target = base.with_extension("target");
    fs::create_dir(&base).unwrap();
    fs::set_permissions(&base, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(&target).unwrap();
    symlink(&target, base.join("elio")).unwrap();
    let result = prepare_private_root(&base, unsafe { libc::geteuid() });
    fs::remove_dir_all(&base).unwrap();
    fs::remove_dir_all(&target).unwrap();
    assert!(matches!(result, Err(ProtocolError::InvalidState(_))));
}

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
#[test]
fn cleanup_is_idempotent() {
    let mut endpoint = ServiceEndpoint::bind().unwrap();
    let request_dir = endpoint.socket_path().parent().unwrap().to_path_buf();
    endpoint.cleanup().unwrap();
    endpoint.cleanup().unwrap();
    assert!(!request_dir.exists());
}
