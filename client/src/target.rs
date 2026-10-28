//! \[DESIGN\] Where a server is, and how a client reaches it (RULES.md
//! D42): over TCP, a Unix socket, or TLS. A connection made to a target
//! remembers it, so it can be reached again to cancel a statement.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};

use crate::connection::{Connection, Mechanism, Result, failed};

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
        let unreachable = |e: std::io::Error| failed(&format!("[DESIGN] cannot connect: {e}"));
        match self {
            Target::Tcp(address) => {
                let stream = TcpStream::connect(address).map_err(unreachable)?;
                stream.set_nodelay(true).map_err(|e| failed(&format!("[DESIGN] cannot set the connection up: {e}")))?;
                Ok(Box::new(stream))
            }
            #[cfg(unix)]
            Target::Local(path) => Ok(Box::new(std::os::unix::net::UnixStream::connect(path).map_err(unreachable)?)),
            #[cfg(feature = "tls")]
            Target::Tls(address, name, config) => Ok(Box::new(crate::tls::secured(*address, (name, config))?)),
        }
    }
}

/// Connects to the server at `target` as `user`, authenticated by
/// `mechanism`.
///
/// # Errors
///
/// As `Connection::open`; those of `Target::stream`.
pub fn connect_to(target: &Target, user: &str, mechanism: &mut dyn Mechanism) -> Result<Connection<Box<dyn Duplex>>> {
    Ok(Connection::open(target.stream()?, user, mechanism)?.reached_by(target.clone()))
}
