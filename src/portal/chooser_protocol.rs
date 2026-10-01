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

#[cfg(test)]
#[path = "tests/chooser_protocol.rs"]
mod tests;

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
