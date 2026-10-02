//! Windows Recycle Bin support.
//!
//! The Recycle Bin stores each deleted item as a pair of files in
//! `<volume>\$Recycle.Bin\<user SID>\`:
//!
//! - `$R<token>[.ext]` — the actual file or directory contents.
//! - `$I<token>[.ext]` — a fixed-layout metadata sidecar recording the
//!   original path, the original size, and the deletion time.
//!
//! This is the same role the `.trashinfo` sidecar plays in the freedesktop
//! trash spec, so the browsing and restore paths mirror the freedesktop ones
//! in [`super::directory_scanning`] and [`super::trash_restoration`].
//!
//! **Known limitation:** the Recycle Bin is per-volume, and [`recycle_bin_dir`]
//! resolves only the one on the system drive. Items deleted from another
//! volume live in that volume's own `$Recycle.Bin` and are not listed here.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Metadata recovered from a `$I` sidecar.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RecycleInfo {
    /// Absolute path the item occupied before deletion.
    pub original_path: PathBuf,
    /// Size of the original item in bytes, as recorded at deletion time.
    pub original_size: u64,
    /// When the item was deleted. `None` if the timestamp is unrepresentable.
    pub deleted_at: Option<SystemTime>,
}

impl RecycleInfo {
    /// The item's original base name, e.g. `report.pdf`.
    pub fn original_name(&self) -> Option<String> {
        self.original_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
    }
}

/// Returns the current user's Recycle Bin directory on the system drive, or
/// `None` if the SID cannot be resolved or the directory does not exist.
///
/// The directory is only created by Windows once the user has deleted
/// something, so `None` is normal on a fresh profile.
#[cfg(windows)]
pub(crate) fn recycle_bin_dir() -> Option<PathBuf> {
    let sid = current_user_sid()?;
    // %SystemDrive% is normally "C:"; fall back to that if it is unset.
    let system_drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_string());
    let path = PathBuf::from(format!("{system_drive}\\$Recycle.Bin")).join(sid);
    path.is_dir().then_some(path)
}

#[cfg(not(windows))]
pub(crate) fn recycle_bin_dir() -> Option<PathBuf> {
    None
}

/// Returns `true` when `dir` is the current user's Recycle Bin directory.
pub(crate) fn is_recycle_bin_dir(dir: &Path) -> bool {
    recycle_bin_dir().is_some_and(|bin| same_dir(dir, &bin))
}

/// Compares two paths for pointing at the same directory, without touching the
/// filesystem.
///
/// Navigation canonicalizes the directory it loads, and on Windows
/// [`Path::canonicalize`] returns an extended-length path (`\\?\C:\…`), while
/// [`recycle_bin_dir`] builds a plain one from `%SystemDrive%`. A bare `==`
/// therefore never matched once the user actually navigated into the bin: the
/// listing hook was skipped and every item kept its raw `$R…` name, and
/// [`is_recycle_bin_entry`] rejected the very entries restore is for.
///
/// Comparing case-insensitively is correct here rather than merely convenient —
/// Windows paths are case-insensitive, so `C:\` and `c:\` are the same
/// directory. Canonicalization is deliberately not used: this runs once per
/// entry from [`is_recycle_bin_entry`], and a syscall per item would show up on
/// a bin holding thousands.
#[cfg(windows)]
fn same_dir(a: &Path, b: &Path) -> bool {
    fn comparable(path: &Path) -> String {
        let text = path.to_string_lossy();
        let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
        text.trim_end_matches('\\').to_lowercase()
    }

    comparable(a) == comparable(b)
}

#[cfg(not(windows))]
fn same_dir(a: &Path, b: &Path) -> bool {
    a == b
}

/// Returns `true` when `path` is a `$R…` content entry sitting directly in the
/// current user's Recycle Bin.
///
/// Restore uses this to decide whether the Recycle Bin backend applies at all,
/// so that an ordinary file elsewhere on disk still reports the layout as
/// unsupported instead of failing with a confusing "missing sidecar" error.
#[cfg(windows)]
pub(crate) fn is_recycle_bin_entry(path: &Path) -> bool {
    let is_content = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            let bytes = name.as_bytes();
            bytes.len() >= 2 && bytes[0] == b'$' && bytes[1].eq_ignore_ascii_case(&b'R')
        });
    is_content && path.parent().is_some_and(is_recycle_bin_dir)
}

/// Returns `true` for the `$I` metadata sidecars, which are an implementation
/// detail and must never be shown to the user as entries in their own right.
pub(crate) fn is_info_sidecar_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() >= 2 && bytes[0] == b'$' && bytes[1].eq_ignore_ascii_case(&b'I')
}

/// Maps a `$R…` content path to its paired `$I…` sidecar path.
///
/// The two names differ only in the second character, so `$R0A5E05.txt`
/// pairs with `$I0A5E05.txt`.
pub(crate) fn info_path_for(content_path: &Path) -> Option<PathBuf> {
    let name = content_path.file_name()?.to_str()?;
    let bytes = name.as_bytes();
    if bytes.len() < 2 || bytes[0] != b'$' || !bytes[1].eq_ignore_ascii_case(&b'R') {
        return None;
    }
    let info_name = format!("$I{}", &name[2..]);
    Some(content_path.with_file_name(info_name))
}

/// Reads and parses the `$I` sidecar paired with `content_path`.
pub(crate) fn read_info(content_path: &Path) -> Option<RecycleInfo> {
    let info_path = info_path_for(content_path)?;
    parse_info(&fs::read(info_path).ok()?)
}

/// Deletes the `$I` sidecar paired with `content_path`. Best-effort: a
/// missing or unreadable sidecar is not an error, since the content file is
/// already gone by the time this is called.
#[cfg(windows)]
pub(crate) fn remove_info_sidecar(content_path: &Path) {
    if let Some(info_path) = info_path_for(content_path) {
        let _ = fs::remove_file(info_path);
    }
}

// ---------------------------------------------------------------------------
// $I sidecar format
// ---------------------------------------------------------------------------
// Both versions share a 24-byte prefix:
//
//   0..8    u64 LE   format version (1 = Vista..8.1, 2 = Windows 10+)
//   8..16   u64 LE   original size in bytes
//   16..24  u64 LE   deletion time as a FILETIME
//
// Version 1 then stores the original path as a fixed 260-code-unit
// (520-byte) NUL-padded UTF-16LE buffer.  Version 2 stores a u32 LE count of
// UTF-16 code units (including the NUL terminator) followed by that many
// code units.
// ---------------------------------------------------------------------------

const INFO_HEADER_LEN: usize = 24;
const V1_PATH_CODE_UNITS: usize = 260;

/// Parses the raw bytes of a `$I` sidecar. Returns `None` if the buffer is
/// truncated, uses an unknown version, or holds an empty path.
pub(crate) fn parse_info(bytes: &[u8]) -> Option<RecycleInfo> {
    if bytes.len() < INFO_HEADER_LEN {
        return None;
    }
    let version = u64::from_le_bytes(bytes[0..8].try_into().ok()?);
    let original_size = u64::from_le_bytes(bytes[8..16].try_into().ok()?);
    let filetime = u64::from_le_bytes(bytes[16..24].try_into().ok()?);

    let units: Vec<u16> = match version {
        1 => read_utf16le(&bytes[INFO_HEADER_LEN..], V1_PATH_CODE_UNITS)?,
        2 => {
            if bytes.len() < INFO_HEADER_LEN + 4 {
                return None;
            }
            let count = u32::from_le_bytes(bytes[24..28].try_into().ok()?) as usize;
            read_utf16le(&bytes[INFO_HEADER_LEN + 4..], count)?
        }
        _ => return None,
    };

    // The stored path is NUL-terminated; trim at the first NUL.
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    let path = String::from_utf16(&units[..end]).ok()?;
    if path.is_empty() {
        return None;
    }

    Some(RecycleInfo {
        original_path: PathBuf::from(path),
        original_size,
        deleted_at: filetime_to_system_time(filetime),
    })
}

/// Reads exactly `count` UTF-16LE code units from the front of `bytes`.
fn read_utf16le(bytes: &[u8], count: usize) -> Option<Vec<u16>> {
    let needed = count.checked_mul(2)?;
    if bytes.len() < needed {
        return None;
    }
    Some(
        bytes[..needed]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| u16::from_le_bytes(c))
            .collect(),
    )
}

/// Seconds between the FILETIME epoch (1601-01-01) and the Unix epoch.
const FILETIME_TO_UNIX_SECS: u64 = 11_644_473_600;

/// Converts a Windows FILETIME (100-nanosecond ticks since 1601-01-01 UTC)
/// into a [`SystemTime`]. Returns `None` for timestamps before the Unix
/// epoch, which the Recycle Bin should never produce.
fn filetime_to_system_time(filetime: u64) -> Option<SystemTime> {
    let secs = filetime / 10_000_000;
    let nanos = (filetime % 10_000_000) * 100;
    let unix_secs = secs.checked_sub(FILETIME_TO_UNIX_SECS)?;
    UNIX_EPOCH.checked_add(Duration::new(unix_secs, nanos as u32))
}

// ---------------------------------------------------------------------------
// Current user SID
// ---------------------------------------------------------------------------

/// Returns the current user's SID in string form (`S-1-5-21-…`).
///
/// Cached after the first successful lookup — the SID cannot change for the
/// lifetime of the process, and this is called on every sidebar rebuild.
#[cfg(windows)]
fn current_user_sid() -> Option<&'static str> {
    use std::sync::OnceLock;
    static SID: OnceLock<Option<String>> = OnceLock::new();
    SID.get_or_init(query_current_user_sid).as_deref()
}

#[cfg(windows)]
mod ffi {
    use std::ffi::c_void;

    pub(super) type Handle = *mut c_void;

    pub(super) const TOKEN_QUERY: u32 = 0x0008;
    /// `TOKEN_INFORMATION_CLASS::TokenUser`
    pub(super) const TOKEN_USER_CLASS: i32 = 1;

    #[link(name = "advapi32")]
    unsafe extern "system" {
        /// Opens the access token associated with a process.
        pub(super) fn OpenProcessToken(
            process: Handle,
            desired_access: u32,
            token: *mut Handle,
        ) -> i32;
        /// Retrieves a specified type of information about an access token.
        pub(super) fn GetTokenInformation(
            token: Handle,
            class: i32,
            info: *mut c_void,
            info_len: u32,
            return_len: *mut u32,
        ) -> i32;
        /// Converts a binary SID to its `S-1-…` string form. The result is
        /// allocated with `LocalAlloc` and must be freed with `LocalFree`.
        pub(super) fn ConvertSidToStringSidW(sid: *mut c_void, out: *mut *mut u16) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub(super) fn GetCurrentProcess() -> Handle;
        pub(super) fn CloseHandle(handle: Handle) -> i32;
        pub(super) fn LocalFree(mem: *mut c_void) -> *mut c_void;
    }
}

/// Queries the current process token for the user SID and formats it as a
/// string. Returns `None` if any Win32 call fails.
#[cfg(windows)]
fn query_current_user_sid() -> Option<String> {
    use ffi::*;
    use std::{ffi::c_void, ptr};

    // SAFETY: each call below is checked for failure before its output is
    // used, the token handle is closed on every path, and the string returned
    // by ConvertSidToStringSidW is freed with LocalFree as documented.
    unsafe {
        let mut token: Handle = ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return None;
        }

        // First call sizes the buffer; it is expected to fail with
        // ERROR_INSUFFICIENT_BUFFER while writing the required length.
        let mut needed: u32 = 0;
        GetTokenInformation(token, TOKEN_USER_CLASS, ptr::null_mut(), 0, &mut needed);
        if needed == 0 {
            CloseHandle(token);
            return None;
        }

        let mut buffer = vec![0u8; needed as usize];
        let ok = GetTokenInformation(
            token,
            TOKEN_USER_CLASS,
            buffer.as_mut_ptr().cast::<c_void>(),
            needed,
            &mut needed,
        );
        CloseHandle(token);
        if ok == 0 {
            return None;
        }

        // TOKEN_USER begins with a SID_AND_ATTRIBUTES whose first field is the
        // PSID pointer, so the SID pointer sits at the front of the buffer.
        if buffer.len() < size_of::<*mut c_void>() {
            return None;
        }
        let sid = buffer.as_ptr().cast::<*mut c_void>().read_unaligned();
        if sid.is_null() {
            return None;
        }

        let mut wide: *mut u16 = ptr::null_mut();
        if ConvertSidToStringSidW(sid, &mut wide) == 0 || wide.is_null() {
            return None;
        }

        let mut len = 0usize;
        while *wide.add(len) != 0 {
            len += 1;
        }
        let sid_string = String::from_utf16(std::slice::from_raw_parts(wide, len)).ok();
        LocalFree(wide.cast::<c_void>());

        sid_string
    }
}

#[cfg(test)]
#[path = "tests/recycle_bin.rs"]
mod tests;
