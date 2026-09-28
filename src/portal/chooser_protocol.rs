//! Private, byte-safe transport for a single portal chooser request.
//!
//! The protocol deliberately has no routing or portal metadata: one listener
//! serves exactly one child chooser and is removed when its endpoint is closed.

use serde::{Deserialize, Serialize};
use std::{fmt, io, time::Instant};

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
use std::{
    env,
    fs::{self, OpenOptions},
    io::{Read, Write},
    net::Shutdown,
    path::{Path, PathBuf},
    time::Duration,
};

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
use std::{
    os::{
        fd::AsRawFd,
        unix::{
            fs::{MetadataExt, PermissionsExt},
            net::{UnixListener, UnixStream},
        },
    },
    thread,
};

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
const MAGIC: [u8; 4] = *b"ELIO";
#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
const VERSION: u16 = 1;
#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
const HEADER_LEN: usize = 10;
pub(crate) const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const MAX_ERROR_CODE_BYTES: usize = 64;

/// A byte-preserving request mode. Keep this independent from the chooser UI;
/// later integration maps it to `chooser::portal::PortalChooserMode`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub(crate) enum ChooserRequestMode {
    Open { kind: SelectionKind, multiple: bool },
    SaveFile { initial_name: String },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SelectionKind {
    File,
    Directory,
    FileOrDirectory,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ChooserRequest {
    pub(crate) mode: ChooserRequestMode,
    /// Native Unix bytes, rather than a lossy string representation.
    pub(crate) initial_path: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct ErrorCode(String);

impl ErrorCode {
    pub(crate) fn new(code: impl Into<String>) -> Result<Self, ProtocolError> {
        let code = code.into();
        if code.is_empty() || code.len() > MAX_ERROR_CODE_BYTES || !code.is_ascii() {
            return Err(ProtocolError::InvalidErrorCode);
        }
        Ok(Self(code))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ErrorCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let code = String::deserialize(deserializer)?;
        Self::new(code).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "data")]
pub(crate) enum Message {
    Request(ChooserRequest),
    Ready,
    Accepted { paths: Vec<Vec<u8>> },
    Cancelled,
    Error { code: ErrorCode },
    Cancel,
}

impl Message {
    #[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
    fn is_terminal_result(&self) -> bool {
        matches!(
            self,
            Self::Accepted { .. } | Self::Cancelled | Self::Error { .. }
        )
    }
}

#[derive(Debug)]
pub(crate) enum ProtocolError {
    Io(io::Error),
    Timeout,
    InvalidHeader,
    UnsupportedVersion(u16),
    MessageTooLarge(usize),
    InvalidMessage(String),
    InvalidState(&'static str),
    InvalidErrorCode,
    PeerUidMismatch,
    UnsupportedPlatform,
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "socket I/O failed: {error}"),
            Self::Timeout => f.write_str("portal chooser connection timed out"),
            Self::InvalidHeader => f.write_str("invalid portal chooser protocol header"),
            Self::UnsupportedVersion(version) => {
                write!(f, "unsupported portal chooser protocol version {version}")
            }
            Self::MessageTooLarge(length) => {
                write!(f, "portal chooser message exceeds limit ({length} bytes)")
            }
            Self::InvalidMessage(error) => write!(f, "invalid portal chooser message: {error}"),
            Self::InvalidState(state) => {
                write!(f, "invalid portal chooser protocol state: {state}")
            }
            Self::InvalidErrorCode => f.write_str(
                "portal chooser error code must be non-empty ASCII and at most 64 bytes",
            ),
            Self::PeerUidMismatch => {
                f.write_str("portal chooser peer UID does not match this user")
            }
            Self::UnsupportedPlatform => {
                f.write_str("portal chooser sockets are only supported on Linux and FreeBSD")
            }
        }
    }
}

impl std::error::Error for ProtocolError {}

impl From<io::Error> for ProtocolError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
pub(crate) struct ServiceEndpoint {
    listener: UnixListener,
    socket_path: PathBuf,
    request_dir: PathBuf,
    cleaned: bool,
}

#[cfg(not(all(unix, any(target_os = "linux", target_os = "freebsd"))))]
pub(crate) struct ServiceEndpoint;

impl ServiceEndpoint {
    /// Creates a private listener under a secure runtime/state directory.
    #[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
    pub(crate) fn bind() -> Result<Self, ProtocolError> {
        let root = private_root()?;
        let request_dir = root.join(random_component()?);
        fs::create_dir(&request_dir)?;
        fs::set_permissions(&request_dir, fs::Permissions::from_mode(0o700))?;
        let socket_path = request_dir.join("request.sock");
        let listener = match UnixListener::bind(&socket_path) {
            Ok(listener) => listener,
            Err(error) => {
                let _ = fs::remove_dir_all(&request_dir);
                return Err(error.into());
            }
        };
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            socket_path,
            request_dir,
            cleaned: false,
        })
    }

    #[cfg(not(all(unix, any(target_os = "linux", target_os = "freebsd"))))]
    pub(crate) fn bind() -> Result<Self, ProtocolError> {
        Err(ProtocolError::UnsupportedPlatform)
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
    pub(crate) fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Wait for a validated child connection until `deadline` expires.
    #[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
    pub(crate) fn accept_until(
        &self,
        deadline: Instant,
    ) -> Result<ServiceConnection, ProtocolError> {
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    validate_peer_uid(&stream)?;
                    stream.set_nonblocking(false)?;
                    return Ok(ServiceConnection {
                        stream,
                        state: ServiceState::Connected,
                        request: None,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(ProtocolError::Timeout);
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    #[cfg(not(all(unix, any(target_os = "linux", target_os = "freebsd"))))]
    pub(crate) fn accept_until(&self, _: Instant) -> Result<ServiceConnection, ProtocolError> {
        Err(ProtocolError::UnsupportedPlatform)
    }

    /// Safe to call repeatedly, including after a disconnected child.
    #[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
    pub(crate) fn cleanup(&mut self) -> Result<(), ProtocolError> {
        if !self.cleaned {
            self.cleaned = true;
            match fs::remove_dir_all(&self.request_dir) {
                Ok(()) => (),
                Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    #[cfg(not(all(unix, any(target_os = "linux", target_os = "freebsd"))))]
    pub(crate) fn cleanup(&mut self) -> Result<(), ProtocolError> {
        Ok(())
    }
}

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
impl Drop for ServiceEndpoint {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ServiceState {
    Connected,
    Requested,
    Ready,
    Result,
}

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
pub(crate) struct ServiceConnection {
    stream: UnixStream,
    state: ServiceState,
    request: Option<ChooserRequest>,
}

/// A write-only cancellation handle that can be moved to the future request
/// owner while `ServiceConnection::receive_result()` blocks in another task.
#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
pub(crate) struct ServiceCancellation {
    stream: UnixStream,
    sent: bool,
}

#[cfg(not(all(unix, any(target_os = "linux", target_os = "freebsd"))))]
pub(crate) struct ServiceConnection;

impl ServiceConnection {
    #[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
    pub(crate) fn set_timeout(&self, timeout: Option<Duration>) -> Result<(), ProtocolError> {
        self.stream.set_read_timeout(timeout)?;
        self.stream.set_write_timeout(timeout)?;
        Ok(())
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
    pub(crate) fn send_request(&mut self, request: &ChooserRequest) -> Result<(), ProtocolError> {
        if self.state != ServiceState::Connected {
            return Err(ProtocolError::InvalidState("request already sent"));
        }
        write_message(&mut self.stream, &Message::Request(request.clone()))?;
        self.request = Some(request.clone());
        self.state = ServiceState::Requested;
        Ok(())
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
    pub(crate) fn receive_ready(&mut self) -> Result<(), ProtocolError> {
        if self.state != ServiceState::Requested {
            return Err(ProtocolError::InvalidState("expected request before ready"));
        }
        match read_message(&mut self.stream)? {
            Message::Ready => {
                self.state = ServiceState::Ready;
                Ok(())
            }
            _ => Err(ProtocolError::InvalidState("expected ready")),
        }
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
    pub(crate) fn cancellation_handle(&self) -> Result<ServiceCancellation, ProtocolError> {
        if self.state != ServiceState::Ready {
            return Err(ProtocolError::InvalidState("cancel requires ready chooser"));
        }
        Ok(ServiceCancellation {
            stream: self.stream.try_clone()?,
            sent: false,
        })
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
    pub(crate) fn receive_result(&mut self) -> Result<Message, ProtocolError> {
        if self.state != ServiceState::Ready {
            return Err(ProtocolError::InvalidState("expected ready before result"));
        }
        let message = read_message(&mut self.stream)?;
        if !message.is_terminal_result() {
            return Err(ProtocolError::InvalidState("expected terminal result"));
        }
        validate_result(
            self.request
                .as_ref()
                .ok_or(ProtocolError::InvalidState("missing chooser request"))?,
            &message,
        )?;
        self.state = ServiceState::Result;
        Ok(message)
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
    pub(crate) fn send_cancel(&mut self) -> Result<(), ProtocolError> {
        if self.state != ServiceState::Ready {
            return Err(ProtocolError::InvalidState("cancel requires ready chooser"));
        }
        write_message(&mut self.stream, &Message::Cancel)
    }
}

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
impl ServiceCancellation {
    pub(crate) fn send_cancel(&mut self) -> Result<(), ProtocolError> {
        if self.sent {
            return Err(ProtocolError::InvalidState("cancellation already sent"));
        }
        write_message(&mut self.stream, &Message::Cancel)?;
        self.sent = true;
        Ok(())
    }

    /// Ends the service side after cancellation so an uncooperative child
    /// cannot leave the request worker blocked on its terminal result.
    pub(crate) fn cancel_and_disconnect(&mut self) -> Result<(), ProtocolError> {
        self.send_cancel()?;
        self.stream.shutdown(Shutdown::Both)?;
        Ok(())
    }
}

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ChildState {
    Connected,
    Requested,
    Ready,
    Result,
}

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
pub(crate) struct ChildEndpoint {
    stream: UnixStream,
    state: ChildState,
}

#[cfg(not(all(unix, any(target_os = "linux", target_os = "freebsd"))))]
pub(crate) struct ChildEndpoint;

impl ChildEndpoint {
    #[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
    pub(crate) fn connect_until(path: &Path, deadline: Instant) -> Result<Self, ProtocolError> {
        loop {
            match UnixStream::connect(path) {
                Ok(stream) => {
                    validate_peer_uid(&stream)?;
                    return Ok(Self {
                        stream,
                        state: ChildState::Connected,
                    });
                }
                Err(error) if retryable_connect_error(&error) && Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(5))
                }
                Err(error) if retryable_connect_error(&error) => {
                    return Err(ProtocolError::Timeout);
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
    pub(crate) fn set_timeout(&self, timeout: Option<Duration>) -> Result<(), ProtocolError> {
        self.stream.set_read_timeout(timeout)?;
        self.stream.set_write_timeout(timeout)?;
        Ok(())
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
    pub(crate) fn receive_request(&mut self) -> Result<ChooserRequest, ProtocolError> {
        if self.state != ChildState::Connected {
            return Err(ProtocolError::InvalidState("request already received"));
        }
        match read_message(&mut self.stream)? {
            Message::Request(request) => {
                self.state = ChildState::Requested;
                Ok(request)
            }
            _ => Err(ProtocolError::InvalidState("expected request")),
        }
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
    pub(crate) fn send_ready(&mut self) -> Result<(), ProtocolError> {
        if self.state != ChildState::Requested {
            return Err(ProtocolError::InvalidState("ready requires request"));
        }
        write_message(&mut self.stream, &Message::Ready)?;
        self.state = ChildState::Ready;
        Ok(())
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
    pub(crate) fn receive_cancel(&mut self) -> Result<(), ProtocolError> {
        if self.state != ChildState::Ready {
            return Err(ProtocolError::InvalidState("cancel requires ready chooser"));
        }
        match read_message(&mut self.stream)? {
            Message::Cancel => Ok(()),
            _ => Err(ProtocolError::InvalidState("expected cancel")),
        }
    }

    /// Watches the service half of this connection for a single cancellation.
    /// The listener exits on cancellation, disconnect, or a malformed service frame.
    #[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
    pub(crate) fn watch_for_cancel(
        &self,
        cancellation: crate::chooser::portal::ExternalCancellation,
    ) -> Result<(), ProtocolError> {
        if self.state != ChildState::Ready {
            return Err(ProtocolError::InvalidState("cancel requires ready chooser"));
        }
        let mut stream = self.stream.try_clone()?;
        thread::spawn(move || {
            // A lost service cannot complete this request, so fail closed instead
            // of leaving a terminal chooser orphaned.
            let _ = read_message(&mut stream);
            cancellation.cancel();
        });
        Ok(())
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
    pub(crate) fn send_result(&mut self, result: Message) -> Result<(), ProtocolError> {
        if self.state != ChildState::Ready {
            return Err(ProtocolError::InvalidState(
                "result already sent or chooser not ready",
            ));
        }
        if !result.is_terminal_result() {
            return Err(ProtocolError::InvalidState(
                "result must be accepted, cancelled, or error",
            ));
        }
        write_message(&mut self.stream, &result)?;
        self.state = ChildState::Result;
        self.stream.shutdown(Shutdown::Write)?;
        Ok(())
    }
}

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
fn validate_result(request: &ChooserRequest, message: &Message) -> Result<(), ProtocolError> {
    let Message::Accepted { paths } = message else {
        return Ok(());
    };
    if paths.is_empty() {
        return Err(ProtocolError::InvalidState("accepted result has no paths"));
    }
    let valid_cardinality = match &request.mode {
        ChooserRequestMode::Open { multiple, .. } => *multiple || paths.len() == 1,
        ChooserRequestMode::SaveFile { .. } => paths.len() == 1,
    };
    valid_cardinality
        .then_some(())
        .ok_or(ProtocolError::InvalidState(
            "accepted result has invalid cardinality",
        ))
}

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
fn write_message(stream: &mut UnixStream, message: &Message) -> Result<(), ProtocolError> {
    let payload = serde_json::to_vec(message)
        .map_err(|error| ProtocolError::InvalidMessage(error.to_string()))?;
    if payload.len() > MAX_MESSAGE_BYTES {
        return Err(ProtocolError::MessageTooLarge(payload.len()));
    }
    let length =
        u32::try_from(payload.len()).map_err(|_| ProtocolError::MessageTooLarge(payload.len()))?;
    stream.write_all(&MAGIC)?;
    stream.write_all(&VERSION.to_be_bytes())?;
    stream.write_all(&length.to_be_bytes())?;
    stream.write_all(&payload)?;
    Ok(())
}

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
fn read_message(stream: &mut UnixStream) -> Result<Message, ProtocolError> {
    let mut header = [0; HEADER_LEN];
    stream.read_exact(&mut header)?;
    if header[..4] != MAGIC {
        return Err(ProtocolError::InvalidHeader);
    }
    let version = u16::from_be_bytes([header[4], header[5]]);
    if version != VERSION {
        return Err(ProtocolError::UnsupportedVersion(version));
    }
    let length = u32::from_be_bytes([header[6], header[7], header[8], header[9]]) as usize;
    if length > MAX_MESSAGE_BYTES {
        return Err(ProtocolError::MessageTooLarge(length));
    }
    let mut payload = vec![0; length];
    stream.read_exact(&mut payload)?;
    serde_json::from_slice(&payload)
        .map_err(|error| ProtocolError::InvalidMessage(error.to_string()))
}

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
fn private_root() -> Result<PathBuf, ProtocolError> {
    let uid = unsafe { libc::geteuid() };
    let candidates = [
        env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
        env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(dirs::state_dir),
        Some(env::temp_dir().join(format!("elio-{uid}"))),
    ];
    for candidate in candidates.into_iter().flatten() {
        if let Ok(root) = prepare_private_root(&candidate, uid) {
            return Ok(root);
        }
    }
    Err(ProtocolError::InvalidState(
        "could not create a secure portal runtime directory",
    ))
}

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
fn prepare_private_root(base: &Path, uid: libc::uid_t) -> Result<PathBuf, ProtocolError> {
    ensure_private_base(base, uid)?;
    let elio = base.join("elio");
    ensure_private_directory(&elio, uid)?;
    let root = elio.join("portal");
    ensure_private_directory(&root, uid)?;
    Ok(root)
}

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
fn ensure_private_base(path: &Path, uid: libc::uid_t) -> Result<(), ProtocolError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => validate_private_base(&metadata, uid),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(path)?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
            validate_private_base(&fs::symlink_metadata(path)?, uid)
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
fn validate_private_base(metadata: &fs::Metadata, uid: libc::uid_t) -> Result<(), ProtocolError> {
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != uid
        || metadata.mode() & 0o022 != 0
    {
        return Err(ProtocolError::InvalidState(
            "unsafe portal runtime directory",
        ));
    }
    Ok(())
}

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
fn ensure_private_directory(path: &Path, uid: libc::uid_t) -> Result<(), ProtocolError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => validate_private_directory(&metadata, uid),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            match fs::create_dir(path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
            validate_private_directory(&fs::symlink_metadata(path)?, uid)
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
fn validate_private_directory(
    metadata: &fs::Metadata,
    uid: libc::uid_t,
) -> Result<(), ProtocolError> {
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != uid
        || metadata.mode() & 0o077 != 0
    {
        return Err(ProtocolError::InvalidState(
            "unsafe portal runtime directory",
        ));
    }
    Ok(())
}

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
fn random_component() -> Result<String, ProtocolError> {
    let mut bytes = [0u8; 16];
    OpenOptions::new()
        .read(true)
        .open("/dev/urandom")?
        .read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(all(unix, any(target_os = "linux", target_os = "freebsd")))]
fn retryable_connect_error(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
    )
}

#[cfg(target_os = "linux")]
fn validate_peer_uid(stream: &UnixStream) -> Result<(), ProtocolError> {
    let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error().into());
    }
    if length != std::mem::size_of::<libc::ucred>() as libc::socklen_t
        || credentials.uid != unsafe { libc::geteuid() }
    {
        return Err(ProtocolError::PeerUidMismatch);
    }
    Ok(())
}

#[cfg(target_os = "freebsd")]
fn validate_peer_uid(stream: &UnixStream) -> Result<(), ProtocolError> {
    let mut euid = 0;
    let mut egid = 0;
    let result = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut euid, &mut egid) };
    if result != 0 {
        return Err(io::Error::last_os_error().into());
    }
    if euid != unsafe { libc::geteuid() } {
        return Err(ProtocolError::PeerUidMismatch);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
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
                ChildEndpoint::connect_until(&socket, Instant::now() + Duration::from_secs(1))
                    .unwrap();
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
                ChildEndpoint::connect_until(&socket, Instant::now() + Duration::from_secs(1))
                    .unwrap();
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
                ChildEndpoint::connect_until(&socket, Instant::now() + Duration::from_secs(1))
                    .unwrap();
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
                ChildEndpoint::connect_until(&socket, Instant::now() + Duration::from_secs(1))
                    .unwrap();
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
                ChildEndpoint::connect_until(&socket, Instant::now() + Duration::from_secs(1))
                    .unwrap();
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
                ChildEndpoint::connect_until(&socket, Instant::now() + Duration::from_secs(1))
                    .unwrap();
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
}
