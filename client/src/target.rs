//! \[DESIGN\] Where a server is, and how a client reaches it (RULES.md
//! D42): over TCP, a Unix socket, or TLS. A connection made to a target
//! remembers it, so it can be reached again to cancel a statement.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use crate::connection::{Connection, Mechanism, Result, failed, in_the_clear};
use crate::timeouts::{Socket, Timeouts, io_failed};

/// A stream that reads and writes, of any transport.
pub trait Duplex: Read + Write + Send {}

impl<T: Read + Write + Send> Duplex for T {}

/// Where a server is, and how it is reached.
#[derive(Clone)]
pub enum Target {
    /// Over TCP, at this address.
    Tcp(SocketAddr),
    /// Over the Unix socket at this path.
    #[cfg(unix)]
    Local(std::path::PathBuf),
    /// Over TLS, at this address, the server of this name, trusted as this
    /// configuration says.
    #[cfg(feature = "tls")]
    Tls(SocketAddr, String, std::sync::Arc<rustls::ClientConfig>),
}

impl std::fmt::Debug for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Target::Tcp(address) => write!(f, "Tcp({address})"),
            #[cfg(unix)]
            Target::Local(path) => write!(f, "Local({})", path.display()),
            #[cfg(feature = "tls")]
            Target::Tls(address, name, _) => write!(f, "Tls({address}, {name})"),
        }
    }
}

impl Target {
    /// A stream to the server: TLS's handshake done.
    ///
    /// # Errors
    ///
    /// A connection failure if it cannot be reached, or TLS's handshake
    /// fails.
    pub fn stream(&self) -> Result<Box<dyn Duplex>> {
        self.stream_within(None)
    }

    /// A stream to the server, as `stream`, made, and then read and
    /// written, each within `limit` if given, else without one.
    ///
    /// # Errors
    ///
    /// As `stream`'s; a connection failure if it is not made within
    /// `limit`.
    pub fn stream_within(&self, limit: Option<Duration>) -> Result<Box<dyn Duplex>> {
        self.opened(limit).map(|(stream, _)| stream)
    }

    /// A stream to the server, as `stream_within`, and a handle on its
    /// socket, to bound its waits later.
    ///
    /// # Errors
    ///
    /// As `stream_within`'s.
    pub(crate) fn opened(&self, limit: Option<Duration>) -> Result<(Box<dyn Duplex>, Socket)> {
        let set_up = |e: std::io::Error| failed(&format!("[DESIGN] cannot set the connection up: {e}"));
        match self {
            Target::Tcp(address) => {
                let stream = connected(address, limit)?;
                stream.set_nodelay(true).map_err(set_up)?;
                let socket = Socket::tcp(&stream)?;
                Ok((Box::new(stream), socket))
            }
            #[cfg(unix)]
            Target::Local(path) => {
                let stream = std::os::unix::net::UnixStream::connect(path).map_err(|e| io_failed("connect", &e))?;
                let socket = Socket::local(&stream)?;
                socket.limit(limit)?;
                Ok((Box::new(stream), socket))
            }
            #[cfg(feature = "tls")]
            Target::Tls(address, name, config) => {
                let tcp = connected(address, limit)?;
                let socket = Socket::tcp(&tcp)?;
                Ok((Box::new(crate::tls::secured(tcp, (name, config))?), socket))
            }
        }
    }
}

/// A TCP connection to `address`, made, read and written each within
/// `limit` if given.
///
/// # Errors
///
/// A connection failure if it cannot be made, in time.
fn connected(address: &SocketAddr, limit: Option<Duration>) -> Result<TcpStream> {
    let stream = match limit {
        Some(limit) => TcpStream::connect_timeout(address, limit),
        None => TcpStream::connect(address),
    }
    .map_err(|e| io_failed("connect", &e))?;
    stream
        .set_read_timeout(limit)
        .and_then(|()| stream.set_write_timeout(limit))
        .map_err(|e| failed(&format!("[DESIGN] cannot set the connection up: {e}")))?;
    Ok(stream)
}

/// Connects to the server at `target` as `user`, authenticated by
/// `mechanism`, waiting as `Timeouts::default` says.
///
/// # Errors
///
/// As `connect_to_within`'s.
pub fn connect_to(target: &Target, user: &str, mechanism: &mut dyn Mechanism) -> Result<Connection<Box<dyn Duplex>>> {
    connect_to_within(target, user, mechanism, Timeouts::default())
}

/// Connects to the server at `target` as `user`, authenticated by
/// `mechanism`, waiting on the server as `timeouts` say: to connect, to
/// begin TLS and for each step of the login, `connect`; for each read or
/// write after, `read`.
///
/// # Errors
///
/// As `Connection::open`; those of `Target::stream`; a connection failure
/// if a wait runs out (SQLSTATE 08006); SQLSTATE 28000, before a
/// connection is made, for a secret mechanism, as `Password`, to a
/// `Target::Tcp`.
pub fn connect_to_within(
    target: &Target,
    user: &str,
    mechanism: &mut dyn Mechanism,
    timeouts: Timeouts,
) -> Result<Connection<Box<dyn Duplex>>> {
    if matches!(target, Target::Tcp(_)) {
        in_the_clear(mechanism)?;
    }
    let (stream, socket) = target.opened(timeouts.connect)?;
    Ok(Connection::open_timed(stream, socket, (user, mechanism), timeouts)?.reached_by(target.clone()))
}
