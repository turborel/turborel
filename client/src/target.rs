//! \[DESIGN\] Where a server is, and how a client reaches it (RULES.md
//! D42): over TCP, a Unix socket (on Windows, a named pipe), or TLS. A
//! connection made to a target remembers it, so it can be reached again to
//! cancel a statement.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use crate::connection::{Connection, Mechanism, Result, TlsChannel, failed, in_the_clear};
#[cfg(unix)]
use crate::timeouts::limit;
use crate::timeouts::{Bound, Timeouts, io_failed};
use crate::told::Offer;

/// A stream that reads and writes, of any transport.
pub trait Duplex: Read + Write + Send + Bound {}

impl<T: Read + Write + Send + Bound> Duplex for T {}

impl Bound for Box<dyn Duplex> {
    fn bound(&self, within: Option<Duration>) -> std::io::Result<()> {
        (**self).bound(within)
    }
    fn gone(&self) -> bool {
        (**self).gone()
    }
}

/// A stream to a server, and over TLS what its handshake gives a
/// mechanism to bind to.
pub(crate) type Opened = (Box<dyn Duplex>, Option<TlsChannel>);

/// Where a server is, and how it is reached.
#[derive(Clone)]
pub enum Target {
    /// Over TCP, at this address.
    Tcp(SocketAddr),
    /// Over the Unix socket at this path; on Windows, the named pipe of
    /// this name, as `\\.\pipe\turborel`.
    #[cfg(any(unix, windows))]
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
            #[cfg(any(unix, windows))]
            Target::Local(path) => write!(f, "Local({})", path.display()),
            #[cfg(feature = "tls")]
            Target::Tls(address, name, _) => write!(f, "Tls({address}, {name})"),
        }
    }
}

impl Target {
    /// The local socket at `path`: a Unix socket, or on Windows a named
    /// pipe.
    ///
    /// # Errors
    ///
    /// A connection failure on a platform with neither.
    #[cfg(any(unix, windows))]
    pub fn local(path: &std::path::Path) -> Result<Target> {
        Ok(Target::Local(path.to_path_buf()))
    }

    /// Refused, as this platform has no local socket.
    ///
    /// # Errors
    ///
    /// A connection failure.
    #[cfg(not(any(unix, windows)))]
    pub fn local(path: &std::path::Path) -> Result<Target> {
        Err(failed(&format!("[DESIGN] {} names a local socket, and this platform has none", path.display())))
    }

    /// Whether it is a plain TCP connection, over which a secret
    /// mechanism is refused.
    pub fn is_plain_tcp(&self) -> bool {
        matches!(self, Target::Tcp(_))
    }

    /// Whether TLS secures it, so a mechanism may bind to its channel.
    pub fn is_tls(&self) -> bool {
        match self {
            Target::Tcp(_) => false,
            #[cfg(any(unix, windows))]
            Target::Local(_) => false,
            #[cfg(feature = "tls")]
            Target::Tls(..) => true,
        }
    }

    /// A stream to the server, as `opened`; but of a `Target::Tls` whose
    /// server is reached and its TLS handshake fails, the same connection
    /// made again over plain TCP, and that target, as `TlsMode::Prefer`
    /// asks: never twice the wait of a server that cannot be reached.
    ///
    /// # Errors
    ///
    /// As `opened`'s.
    pub(crate) fn opened_preferring(&self, within: Option<Duration>) -> Result<(Target, Opened)> {
        #[cfg(feature = "tls")]
        if let Target::Tls(address, name, config) = self {
            let tcp = connected(address, within)?;
            return match crate::tls::secured(tcp, (name, config)) {
                Ok(stream) => {
                    let channel = crate::tls::channel(&stream);
                    Ok((self.clone(), (Box::new(stream), Some(channel))))
                }
                Err(_) => Target::Tcp(*address).opened(within).map(|opened| (Target::Tcp(*address), opened)),
            };
        }
        self.opened(within).map(|opened| (self.clone(), opened))
    }

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

    /// A stream to the server, as `stream_within`, and over TLS what its
    /// handshake gives a mechanism to bind to (`Mechanism::bind`).
    ///
    /// # Errors
    ///
    /// As `stream_within`'s.
    pub(crate) fn opened(&self, within: Option<Duration>) -> Result<Opened> {
        let set_up = |e: std::io::Error| failed(&format!("[DESIGN] cannot set the connection up: {e}"));
        match self {
            Target::Tcp(address) => {
                let stream = connected(address, within)?;
                stream.set_nodelay(true).map_err(set_up)?;
                Ok((Box::new(stream), None))
            }
            #[cfg(unix)]
            Target::Local(path) => {
                let stream = std::os::unix::net::UnixStream::connect(path).map_err(|e| io_failed("connect", &e))?;
                limit(&stream, within)?;
                Ok((Box::new(stream), None))
            }
            #[cfg(windows)]
            Target::Local(path) => Ok((Box::new(crate::pipe::open(path, within)?), None)),
            #[cfg(feature = "tls")]
            Target::Tls(address, name, config) => {
                let tcp = connected(address, within)?;
                let stream = crate::tls::secured(tcp, (name, config))?;
                let channel = crate::tls::channel(&stream);
                Ok((Box::new(stream), Some(channel)))
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
    let opened = target.opened(timeouts.connect)?;
    log_in(target, opened, (user, mechanism), (timeouts, Offer::default()))
}

/// Logs in over `opened`, a stream to the server at `target`, as `user`,
/// authenticated by `mechanism`, bound to its TLS channel if it has one,
/// asking for what `offer` asks.
///
/// # Errors
///
/// As `Connection::open`'s.
pub(crate) fn log_in(
    target: &Target,
    (stream, channel): Opened,
    (user, mechanism): (&str, &mut dyn Mechanism),
    (timeouts, offer): (Timeouts, Offer),
) -> Result<Connection<Box<dyn Duplex>>> {
    if let Some(secured) = channel {
        mechanism.bind(&secured);
    }
    Ok(Connection::open_timed_offering(stream, (user, mechanism), (timeouts, offer))?.reached_by(target.clone()))
}
