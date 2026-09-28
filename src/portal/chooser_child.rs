//! Hidden `--portal-chooser` child mode.
//!
//! This owns only the private socket handshake and maps its typed request to
//! Elio's existing constrained chooser. The portal service itself is added in a
//! later roadmap step.

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
use super::chooser_protocol::ChooserRequest;
#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
use super::chooser_protocol::{ChildEndpoint, ErrorCode, Message};
#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
use super::chooser_protocol::{ChooserRequestMode, SelectionKind};
#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
use crate::chooser::portal::{PortalChooserMode, PortalSelectionKind};
#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
use crate::{
    chooser::{ChooserExit, portal::ExternalCancellation},
    terminal_runtime,
};
#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
use anyhow::Context;
use anyhow::Result;
use std::path::Path;
#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
pub(crate) fn run(socket: &Path) -> Result<()> {
    let mut endpoint = ChildEndpoint::connect_until(socket, Instant::now() + HANDSHAKE_TIMEOUT)
        .map_err(anyhow::Error::from)
        .context("could not connect to portal chooser service")?;
    endpoint
        .set_timeout(Some(HANDSHAKE_TIMEOUT))
        .map_err(anyhow::Error::from)
        .context("could not configure portal chooser handshake timeout")?;
    let request = endpoint
        .receive_request()
        .map_err(anyhow::Error::from)
        .context("could not receive portal chooser request")?;
    endpoint
        .send_ready()
        .map_err(anyhow::Error::from)
        .context("could not acknowledge portal chooser request")?;
    endpoint
        .set_timeout(None)
        .map_err(anyhow::Error::from)
        .context("could not configure portal chooser socket")?;

    let cancellation = ExternalCancellation::default();
    endpoint
        .watch_for_cancel(cancellation.clone())
        .map_err(anyhow::Error::from)
        .context("could not watch for portal chooser cancellation")?;

    let result = run_request(request, cancellation);
    let message = match result {
        Ok(ChooserExit::Confirmed(paths)) => Message::Accepted {
            paths: paths.into_iter().map(path_bytes).collect(),
        },
        Ok(ChooserExit::Cancelled) => Message::Cancelled,
        Err(_) => Message::Error {
            code: ErrorCode::new("chooser_failed").expect("static error code is valid"),
        },
    };
    endpoint
        .send_result(message)
        .map_err(anyhow::Error::from)
        .context("could not send portal chooser result")
}

#[cfg(not(all(unix, any(target_os = "linux", target_os = "freebsd"))))]
pub(crate) fn run(_: &Path) -> Result<()> {
    anyhow::bail!("portal chooser sockets are only supported on Linux and FreeBSD")
}

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
fn run_request(request: ChooserRequest, cancellation: ExternalCancellation) -> Result<ChooserExit> {
    crate::config::initialize(None)?;
    crate::theme::initialize(None)?;
    terminal_runtime::run_portal_chooser(
        request_mode(request.mode),
        request.initial_path.map(path_from_bytes),
        cancellation,
    )
}

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
fn request_mode(mode: ChooserRequestMode) -> PortalChooserMode {
    match mode {
        ChooserRequestMode::Open { kind, multiple } => PortalChooserMode::Open {
            kind: match kind {
                SelectionKind::File => PortalSelectionKind::File,
                SelectionKind::Directory => PortalSelectionKind::Directory,
                SelectionKind::FileOrDirectory => PortalSelectionKind::FileOrDirectory,
            },
            multiple,
        },
        ChooserRequestMode::SaveFile { initial_name } => {
            PortalChooserMode::SaveFile { initial_name }
        }
    }
}

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
fn path_from_bytes(bytes: Vec<u8>) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;

    PathBuf::from(std::ffi::OsString::from_vec(bytes))
}

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
fn path_bytes(path: PathBuf) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;

    path.as_os_str().as_bytes().to_vec()
}

#[cfg(test)]
mod tests {
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
}
