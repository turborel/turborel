//! \[DESIGN\] A named pipe, the local transport on Windows (RULES.md D42),
//! opened as a file: the standard library's own, so no crate is added
//! (D3). Every instance of a pipe busy, as each serves a client until the
//! server makes the next, the open is tried again, until the connection's
//! limit to connect runs out, as the system's documentation of named pipes
//! recommends. The server may learn who the client is, but never act as
//! it.
//!
//! The standard library offers no limit of time on a file's reads and
//! writes, so a pipe's are not bounded once it is open (`Socket::limit`).

use std::io;
use std::time::{Duration, Instant};

#[cfg(windows)]
use crate::connection::Result;
use crate::timeouts::io_failed;

/// \[DESIGN\] ERROR_PIPE_BUSY, the system's error of a pipe every
/// instance of which serves a client (winerror.h).
pub(crate) const ERROR_PIPE_BUSY: i32 = 231;

/// \[DESIGN\] How long a client waits before it tries a busy pipe again:
/// about as long as a server takes to make its next instance.
const BUSY_RETRY: Duration = Duration::from_millis(10);

/// \[DESIGN\] SECURITY_IDENTIFICATION (winnt.h): the server may identify
/// the client but not impersonate it, so a process holding a pipe's name
/// cannot act as the client that opens it.
#[cfg(windows)]
const SECURITY_IDENTIFICATION: u32 = 1 << 16;

/// The named pipe `path`, opened to read and write, tried again while it
/// is busy, within `within` if given.
///
/// # Errors
///
/// A connection failure if it cannot be opened, in time.
#[cfg(windows)]
pub(crate) fn open(path: &std::path::Path, within: Option<Duration>) -> Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    let open = || std::fs::OpenOptions::new().read(true).write(true).security_qos_flags(SECURITY_IDENTIFICATION).open(path);
    retried(open, within).map_err(|e| io_failed("connect", &e))
}

/// What `open` opens, tried again while it fails as busy (ERROR_PIPE_BUSY),
/// until `within`, if given, has passed: then failed as timed out.
///
/// # Errors
///
/// `open`'s error if not busy; `TimedOut` if busy past `within`.
pub(crate) fn retried<T>(mut open: impl FnMut() -> io::Result<T>, within: Option<Duration>) -> io::Result<T> {
    let began = Instant::now();
    loop {
        let e = match open() {
            Ok(opened) => return Ok(opened),
            Err(e) => e,
        };
        if e.raw_os_error() != Some(ERROR_PIPE_BUSY) {
            return Err(e);
        }
        if within.is_some_and(|limit| began.elapsed() >= limit) {
            return Err(io::Error::new(io::ErrorKind::TimedOut, format!("every instance of the pipe is busy ({e})")));
        }
        std::thread::sleep(BUSY_RETRY);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D42: a pipe busy is tried again until it
    /// opens; one busy past the limit to connect fails as timed out, so is
    /// told as the server not answering in time; any other error fails at
    /// once, without a retry.
    #[test]
    fn a_busy_pipe_is_tried_again_within_the_limit() {
        let mut tries = 0_u32;
        let opened = retried(
            || {
                tries += 1;
                if tries < 3 { Err(io::Error::from_raw_os_error(ERROR_PIPE_BUSY)) } else { Ok(tries) }
            },
            Some(Duration::from_secs(60)),
        );
        assert_eq!(opened.unwrap(), 3_u32);
        let busy = retried(|| Err::<(), _>(io::Error::from_raw_os_error(ERROR_PIPE_BUSY)), Some(Duration::ZERO)).unwrap_err();
        assert_eq!(busy.kind(), io::ErrorKind::TimedOut);
        let told = io_failed("connect", &busy);
        assert!(told.message.contains("the server did not answer in time"), "{told:?}");
        let mut once = 0_u32;
        let gone = retried(
            || {
                once += 1;
                Err::<(), _>(io::Error::from(io::ErrorKind::NotFound))
            },
            None,
        );
        assert_eq!((gone.unwrap_err().kind(), once), (io::ErrorKind::NotFound, 1_u32));
    }
}
