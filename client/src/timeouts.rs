//! \[DESIGN\] How long a client waits on its server (RULES.md D42): to
//! connect, secure the connection and log in, each wait at most
//! `Timeouts::connect`; then each read or write of a request or reply at
//! most `Timeouts::read`, none by default, as a statement may run as long
//! as it must; and Terminate's answer at most `Timeouts::read`, else
//! `TERMINATE_WITHIN`. A server gone without closing, or one that hangs,
//! so never holds a client's thread longer than it says.
//!
//! The limits are the stream's own (`Bound`), set on the stream itself, a
//! stream TLS wraps through it, never through a second handle on its
//! socket. A connection made over a stream given to `Connection::open` is
//! bounded as its maker bounds it. A named pipe, on Windows, is bounded
//! only as it is opened (`pipe.rs`): the standard library sets no limit on
//! a pipe's reads and writes.

use std::io;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::connection::{Result, failed};

/// \[DESIGN\] How long a connection waits by default to connect, and for
/// each step of its login: as long as a person waits for an answer.
pub const CONNECT_WITHIN: Duration = Duration::from_secs(30);

/// \[DESIGN\] How long `Connection::terminate` waits for the server to end
/// the session and close, unless the connection's read timeout says: a
/// commit at its end may take long, a server that hangs no longer than
/// this.
pub const TERMINATE_WITHIN: Duration = Duration::from_secs(60);

/// How long a connection waits on its server; `None`, no limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    /// To connect, to begin TLS, and for each step of its login.
    pub connect: Option<Duration>,
    /// For each read or write of the server once logged in: as a
    /// statement's reply is awaited too, none by default.
    pub read: Option<Duration>,
}

impl Default for Timeouts {
    /// `CONNECT_WITHIN` to connect and log in; no limit after.
    fn default() -> Timeouts {
        Timeouts { connect: Some(CONNECT_WITHIN), read: None }
    }
}

/// \[DESIGN\] A stream that bounds its own waits: each read and write by
/// a limit of time, set on the stream itself, never through a second
/// handle on its socket: on Windows a socket's limits are the handle's,
/// so one set through a duplicate bounds no read through the original.
pub trait Bound {
    /// Bounds each read and write by `within`; none, no limit.
    ///
    /// # Errors
    ///
    /// The system's, if it cannot.
    fn bound(&self, within: Option<Duration>) -> io::Result<()>;
    /// Whether the system says it has no peer any more.
    fn gone(&self) -> bool;
}

impl Bound for TcpStream {
    fn bound(&self, within: Option<Duration>) -> io::Result<()> {
        self.set_read_timeout(within).and_then(|()| self.set_write_timeout(within))
    }
    fn gone(&self) -> bool {
        self.peer_addr().is_err()
    }
}

#[cfg(unix)]
impl Bound for std::os::unix::net::UnixStream {
    fn bound(&self, within: Option<Duration>) -> io::Result<()> {
        self.set_read_timeout(within).and_then(|()| self.set_write_timeout(within))
    }
    fn gone(&self) -> bool {
        self.peer_addr().is_err()
    }
}

/// A named pipe, on Windows: not bounded. Once it is open, it waits on its
/// server as long as it must, as the standard library sets no limit of
/// time on a pipe's reads and writes.
#[cfg(windows)]
impl Bound for std::fs::File {
    fn bound(&self, _within: Option<Duration>) -> io::Result<()> {
        Ok(())
    }
    fn gone(&self) -> bool {
        false
    }
}

/// How a connection bounds its stream's waits: `limit`, of the stream's
/// type; none of a stream given to `Connection::open`, bounded by its
/// maker.
pub(crate) type Limit<S> = fn(&S, Option<Duration>) -> Result<()>;

/// Bounds each read and write of `stream` by `within`; none, no limit. A
/// stream its server has closed needs no bound, and is left as it is
/// (`closed`).
///
/// # Errors
///
/// A connection failure if the system cannot bound it.
pub(crate) fn limit<S: Bound + ?Sized>(stream: &S, within: Option<Duration>) -> Result<()> {
    stream
        .bound(within)
        .or_else(|e| if closed(stream, &e) { Ok(()) } else { Err(e) })
        .map_err(|e| failed(&format!("[DESIGN] cannot bound how long the connection waits: {e}")))
}

/// Whether `e`, of bounding `stream`'s waits, is the system refusing to
/// bound a socket that has no peer any more: as macOS refuses (EINVAL)
/// once a Unix socket's server has closed it, as it may as soon as it has
/// the client's Terminate, or a TCP connection is reset. Nothing waits on
/// such a socket: a read takes what the server sent, then its end or the
/// reset; a write fails at once.
fn closed<S: Bound + ?Sized>(stream: &S, e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::InvalidInput && stream.gone()
}

/// A connection failure of what the client could not do, `what`, for `e`:
/// saying so if the server took longer than the connection waits.
pub(crate) fn io_failed(what: &str, e: &io::Error) -> crate::Failure {
    if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) {
        return failed(&format!("[DESIGN] cannot {what}: the server did not answer in time ({e})"));
    }
    failed(&format!("[DESIGN] cannot {what}: {e}"))
}

/// A TCP connection to `address`, made within `within` if given: each of
/// its addresses tried in turn, until one is connected to.
///
/// # Errors
///
/// A connection failure if none can be connected to, the last's.
pub(crate) fn tcp_to(address: impl ToSocketAddrs, within: Option<Duration>) -> Result<TcpStream> {
    let addresses: Vec<SocketAddr> = address.to_socket_addrs().map_err(|e| io_failed("connect", &e))?.collect();
    let mut last = io::Error::new(io::ErrorKind::InvalidInput, "no address to connect to");
    for at in &addresses {
        let made = match within {
            Some(limit) => TcpStream::connect_timeout(at, limit),
            None => TcpStream::connect(at),
        };
        match made {
            Ok(stream) => return Ok(stream),
            Err(e) => last = e,
        }
    }
    Err(io_failed("connect", &last))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D42: a client waits by default
    /// `CONNECT_WITHIN` to connect and log in, and without limit after; a
    /// read that waits longer than its limit is told as the server not
    /// answering in time; a name with no address connects to nothing.
    #[test]
    fn a_client_waits_as_its_timeouts_say() {
        assert_eq!(Timeouts::default(), Timeouts { connect: Some(CONNECT_WITHIN), read: None });
        let late = io_failed("receive", &io::Error::from(io::ErrorKind::WouldBlock));
        assert!(late.message.contains("the server did not answer in time"), "{late:?}");
        let gone = io_failed("receive", &io::Error::from(io::ErrorKind::ConnectionReset));
        assert!(!gone.message.contains("in time"), "{gone:?}");
        let none: &[SocketAddr] = &[];
        assert!(tcp_to(none, None).is_err());
    }

    /// Verifies: [DESIGN] RULES.md D42: a socket without a peer, a Unix
    /// socket its server has closed or a TCP connection reset, is bounded
    /// without failing, and nothing waits on it: what the server sent is
    /// read, then its end or the reset. Regression test (R-TEST-7): macOS
    /// refuses to set a timeout on such a socket (EINVAL), so a connection
    /// whose server closed it as soon as it had Terminate, before
    /// `Connection::terminate` bounded its wait for the close, failed to
    /// terminate, "cannot set the connection up".
    #[cfg(unix)]
    #[test]
    fn a_socket_without_a_peer_is_bounded() {
        use std::io::{Read, Write};
        let (mut client, mut server) = std::os::unix::net::UnixStream::pair().unwrap();
        server.write_all(b"bye").unwrap();
        drop(server);
        limit(&client, Some(Duration::from_secs(1_u64))).unwrap();
        limit(&client, None).unwrap();
        let mut told = Vec::new();
        client.read_to_end(&mut told).unwrap();
        assert_eq!(told, b"bye");
        let mut reset = reset();
        limit(&reset, Some(Duration::from_secs(1_u64))).unwrap();
        assert_eq!(reset.read(&mut [0_u8; 1]).unwrap(), 0_usize, "its end, after the reset");
    }

    /// A TCP connection its server has reset, closing it with what the
    /// client sent unread; the reset read.
    #[cfg(unix)]
    fn reset() -> TcpStream {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (accepted, _) = listener.accept().unwrap();
        client.write_all(b"unread").unwrap();
        accepted.peek(&mut [0_u8; 1]).unwrap();
        drop(accepted);
        let reset = client.read(&mut [0_u8; 1]);
        assert!(reset.is_err(), "{reset:?}");
        client
    }
}
