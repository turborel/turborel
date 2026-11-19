//! The server's local transport, as SERVER.SOCKET names it (RULES.md
//! D42): a Unix socket, or on Windows a named pipe, which the client
//! library opens as a file.

use std::path::Path;

/// A stream of the local transport.
#[cfg(unix)]
pub type Local = std::os::unix::net::UnixStream;

/// A stream of the local transport.
#[cfg(windows)]
pub type Local = std::fs::File;

/// A stream to the server's local endpoint at `path`.
#[cfg(unix)]
pub fn open(path: &Path) -> std::io::Result<Local> {
    Local::connect(path)
}

/// A stream to the server's local endpoint at `path`, the named pipe
/// opened for reading and writing.
#[cfg(windows)]
pub fn open(path: &Path) -> std::io::Result<Local> {
    std::fs::OpenOptions::new().read(true).write(true).open(path)
}
