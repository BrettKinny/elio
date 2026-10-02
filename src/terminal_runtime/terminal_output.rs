#[cfg(any(unix, windows))]
use std::fs::OpenOptions;
use std::io::{self, Write};

type OutputHandle = Box<dyn Write + Send>;

/// Handle to the terminal itself, bypassing stdout.
///
/// Frames, startup escapes and probes go here, never to stdout: stdout may be
/// the `--chooser-file -` stream. `CONOUT$` is the Windows `/dev/tty`; it keeps
/// pointing at the console when stdout is redirected. Needs no VT-mode call,
/// crossterm's `supports_ansi` already enables it on the screen buffer.
#[cfg(unix)]
fn open_controlling_terminal() -> io::Result<std::fs::File> {
    OpenOptions::new().read(true).write(true).open("/dev/tty")
}

#[cfg(windows)]
fn open_controlling_terminal() -> io::Result<std::fs::File> {
    OpenOptions::new().read(true).write(true).open("CONOUT$")
}

#[cfg(unix)]
pub(super) fn terminal_output_handles() -> io::Result<(OutputHandle, OutputHandle)> {
    let tty = open_controlling_terminal()?;
    Ok((Box::new(tty.try_clone()?), Box::new(tty)))
}

#[cfg(windows)]
pub(super) fn terminal_output_handles() -> io::Result<(OutputHandle, OutputHandle)> {
    // No console (detached process): stdout is all that is left, still start.
    let Ok(console) = open_controlling_terminal() else {
        return Ok((Box::new(io::stdout()), Box::new(io::stdout())));
    };
    let frames = console.try_clone()?;
    Ok((
        Box::new(ConsoleWriter::new(console)),
        Box::new(ConsoleWriter::new(frames)),
    ))
}

/// Writes UTF-8 to a Windows console screen buffer as UTF-16.
///
/// A `CONOUT$` `File` writes through `WriteFile`, which decodes with the
/// console's output code page (CP437 by default), so `│` shows as `Γöé`.
/// `io::stdout` transcodes for `WriteConsoleW` itself; this restores that
/// without `SetConsoleOutputCP`, which is global state that outlives a hard exit.
#[cfg(windows)]
struct ConsoleWriter {
    console: std::fs::File,
    /// False for a redirected `CONOUT$`, which can't take `WriteConsoleW`; those
    /// writes stay byte-for-byte.
    is_console: bool,
    /// Tail of a UTF-8 sequence split across `write` calls.
    partial: Vec<u8>,
}

#[cfg(windows)]
impl ConsoleWriter {
    fn new(console: std::fs::File) -> Self {
        let is_console = console_mode(&console).is_some();
        Self {
            console,
            is_console,
            partial: Vec::new(),
        }
    }

    fn write_console(&mut self, units: &[u16]) -> io::Result<()> {
        use std::ffi::c_void;
        use std::os::windows::io::AsRawHandle;

        unsafe extern "system" {
            fn WriteConsoleW(
                console_output: *mut c_void,
                buffer: *const u16,
                chars_to_write: u32,
                chars_written: *mut u32,
                reserved: *mut c_void,
            ) -> i32;
        }

        // One WriteConsoleW call doesn't reliably take a multi-megabyte Kitty payload.
        const MAX_CHUNK: usize = 8192;
        const HIGH_SURROGATES: std::ops::RangeInclusive<u16> = 0xD800..=0xDBFF;

        let handle = self.console.as_raw_handle();
        let mut offset = 0;
        while offset < units.len() {
            let mut end = (offset + MAX_CHUNK).min(units.len());
            // Don't split a surrogate pair across calls.
            if end < units.len() && HIGH_SURROGATES.contains(&units[end - 1]) {
                end -= 1;
            }
            let chunk = &units[offset..end];
            let mut written: u32 = 0;
            let ok = unsafe {
                WriteConsoleW(
                    handle,
                    chunk.as_ptr(),
                    chunk.len() as u32,
                    &mut written,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            if written == 0 {
                return Err(io::Error::other("WriteConsoleW accepted no characters"));
            }
            offset += written as usize;
        }
        Ok(())
    }
}

#[cfg(windows)]
impl Write for ConsoleWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if !self.is_console {
            return self.console.write(buf);
        }
        let units = encode_console_utf16(&mut self.partial, buf);
        self.write_console(&units)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.is_console {
            return self.console.flush();
        }
        // Frames end on a character boundary, so leftover bytes are malformed.
        if !self.partial.is_empty() {
            self.partial.clear();
            self.write_console(&[char::REPLACEMENT_CHARACTER as u16])?;
        }
        Ok(())
    }
}

/// Console handle's mode, or `None` when the handle is not a console.
#[cfg(windows)]
fn console_mode(file: &std::fs::File) -> Option<u32> {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;

    unsafe extern "system" {
        fn GetConsoleMode(handle: *mut c_void, mode: *mut u32) -> i32;
    }

    let mut mode = 0u32;
    let ok = unsafe { GetConsoleMode(file.as_raw_handle(), &mut mode) };
    (ok != 0).then_some(mode)
}

/// Transcodes `partial` + `buf` to UTF-16, leaving a trailing incomplete UTF-8
/// sequence in `partial`. Malformed bytes become U+FFFD.
#[cfg(windows)]
fn encode_console_utf16(partial: &mut Vec<u8>, buf: &[u8]) -> Vec<u16> {
    let mut bytes = std::mem::take(partial);
    bytes.extend_from_slice(buf);

    let mut units = Vec::with_capacity(bytes.len());
    let mut rest = bytes.as_slice();
    loop {
        match std::str::from_utf8(rest) {
            Ok(text) => {
                units.extend(text.encode_utf16());
                return units;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                if let Ok(text) = std::str::from_utf8(&rest[..valid]) {
                    units.extend(text.encode_utf16());
                }
                match error.error_len() {
                    // Cut off mid-sequence: hold the tail for the next write.
                    None => {
                        partial.extend_from_slice(&rest[valid..]);
                        return units;
                    }
                    Some(len) => {
                        units.push(char::REPLACEMENT_CHARACTER as u16);
                        rest = &rest[valid + len..];
                    }
                }
            }
        }
    }
}

#[cfg(not(any(unix, windows)))]
pub(super) fn terminal_output_handles() -> io::Result<(OutputHandle, OutputHandle)> {
    Ok((Box::new(io::stdout()), Box::new(io::stdout())))
}

/// Sink for an escape-sequence probe whose reply the caller reads back.
/// Falls back to stdout when no terminal can be opened.
#[cfg(any(unix, windows))]
pub(crate) fn probe_writer() -> Box<dyn Write> {
    match open_controlling_terminal() {
        Ok(terminal) => Box::new(terminal),
        Err(_) => Box::new(io::stdout()),
    }
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn probe_writer() -> Box<dyn Write> {
    Box::new(io::stdout())
}

#[cfg(all(test, windows))]
#[path = "tests/terminal_output.rs"]
mod tests;
