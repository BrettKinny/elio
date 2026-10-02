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
    Ok((Box::new(console), Box::new(frames)))
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
