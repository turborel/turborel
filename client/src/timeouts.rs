//! \[DESIGN\] How long a client waits on its server (RULES.md D42): to
//! connect, secure the connection and log in, each wait at most
//! `Timeouts::connect`; then each read or write of a request or reply at
//! most `Timeouts::read`, none by default, as a statement may run as long
//! as it must; and Terminate's answer at most `Timeouts::read`, else
//! `TERMINATE_WITHIN`. A server gone without closing, or one that hangs,
//! so never holds a client's thread longer than it says.
//!
//! The limits are the socket's own, set through a second handle on it, so
//! they reach a stream TLS wraps too. A connection made over a stream
//! given to `Connection::open` is bounded as its maker bounds it.

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

/// A second handle on a connection's socket: to bound how long its reads
/// and writes wait.
#[derive(Debug)]
pub(crate) enum Socket {
    /// A TCP connection's, TLS's among them.
    Tcp(TcpStream),
    /// A Unix socket's.
    #[cfg(unix)]
    Local(std::os::unix::net::UnixStream),
}

impl Socket {
    /// A handle on `stream`'s socket.
    pub(crate) fn tcp(stream: &TcpStream) -> Result<Socket> {
        stream.try_clone().map(Socket::Tcp).map_err(|e| set_up(&e))
    }

    /// A handle on the Unix socket `stream`'s.
    #[cfg(unix)]
    pub(crate) fn local(stream: &std::os::unix::net::UnixStream) -> Result<Socket> {
        stream.try_clone().map(Socket::Local).map_err(|e| set_up(&e))
    }

    /// Bounds each read and write of the socket by `within`; none, no
    /// limit.
    pub(crate) fn limit(&self, within: Option<Duration>) -> Result<()> {
        let set = match self {
            Socket::Tcp(s) => s.set_read_timeout(within).and_then(|()| s.set_write_timeout(within)),
            #[cfg(unix)]
            Socket::Local(s) => s.set_read_timeout(within).and_then(|()| s.set_write_timeout(within)),
        };
        set.map_err(|e| set_up(&e))
    }
}

/// A connection that cannot be set up, for `e`.
fn set_up(e: &io::Error) -> crate::Failure {
    failed(&format!("[DESIGN] cannot set the connection up: {e}"))
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
}
