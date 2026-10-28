//! \[DESIGN\] Transports (RULES.md D42): a `Listener` plug-in accepts
//! connections, each a stream that reads and writes, with the peer it came
//! from. TCP and Unix sockets are built in; TLS (`tls.rs`) secures a
//! listener's streams, in the connection's own task.

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;

use tokio::io::{AsyncRead, AsyncWrite};

use crate::auth::Peer;

/// A connection's stream.
pub trait Transport: AsyncRead + AsyncWrite + Unpin + Send + 'static {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send + 'static> Transport for T {}

/// A connection accepted, and who from.
pub type Accepted = io::Result<(Box<dyn Transport>, Peer)>;

/// What a `Listener` returns from `accept`.
pub type Accepting<'a> = Pin<Box<dyn Future<Output = Accepted> + Send + 'a>>;

/// \[DESIGN\] Where a server takes connections: an extension point
/// (RULES.md D42).
///
/// Contract (RULES.md R-SAFE-3): `accept` waits for the next connection;
/// an error is of that one connection, and the server waits for the next.
/// `address` names where it listens, for the operator.
pub trait Listener: Send + Sync {
    /// The next connection.
    fn accept(&self) -> Accepting<'_>;
    /// Where it listens.
    fn address(&self) -> String;
}

/// \[DESIGN\] The built-in TCP listener.
pub struct Tcp {
    listener: tokio::net::TcpListener,
}

impl Tcp {
    /// Listens at `address`.
    ///
    /// # Errors
    ///
    /// The system's, if it cannot.
    pub async fn bind(address: SocketAddr) -> io::Result<Tcp> {
        Ok(Tcp { listener: tokio::net::TcpListener::bind(address).await? })
    }

    /// The address it listens at: with the port the system chose, if asked
    /// for port 0.
    ///
    /// # Errors
    ///
    /// The system's, if it cannot say.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }
}

impl Listener for Tcp {
    fn accept(&self) -> Accepting<'_> {
        Box::pin(async {
            let (stream, address) = self.listener.accept().await?;
            // Replies are flushed whole, so nothing waits for more to send.
            stream.set_nodelay(true)?;
            let stream: Box<dyn Transport> = Box::new(stream);
            Ok((stream, Peer::tcp(address)))
        })
    }

    fn address(&self) -> String {
        self.local_addr().map_or_else(|e| format!("tcp ({e})"), |a| a.to_string())
    }
}

/// \[DESIGN\] The built-in Unix socket listener. Its socket file is removed
/// when it is dropped; one left by a server that did not stop cleanly is
/// replaced, but not one a server still listens at, nor a file that is
/// not a socket.
#[cfg(unix)]
pub struct Local {
    listener: tokio::net::UnixListener,
    path: std::path::PathBuf,
}

#[cfg(unix)]
impl Local {
    /// Listens at the socket `path`.
    ///
    /// # Errors
    ///
    /// The system's; `AddrInUse` if a server listens there, or `path` is
    /// not a socket.
    pub async fn bind(path: &std::path::Path) -> io::Result<Local> {
        use std::os::unix::fs::FileTypeExt;
        if let Ok(meta) = std::fs::symlink_metadata(path) {
            let in_use = !meta.file_type().is_socket() || tokio::net::UnixStream::connect(path).await.is_ok();
            if in_use {
                return Err(io::Error::new(io::ErrorKind::AddrInUse, format!("{} is in use", path.display())));
            }
            std::fs::remove_file(path)?;
        }
        Ok(Local { listener: tokio::net::UnixListener::bind(path)?, path: path.to_path_buf() })
    }
}

#[cfg(unix)]
impl Listener for Local {
    fn accept(&self) -> Accepting<'_> {
        Box::pin(async {
            let (stream, _) = self.listener.accept().await?;
            let stream: Box<dyn Transport> = Box::new(stream);
            Ok((stream, Peer::local()))
        })
    }

    fn address(&self) -> String {
        self.path.display().to_string()
    }
}

#[cfg(unix)]
impl Drop for Local {
    fn drop(&mut self) {
        // Drop cannot return an error: a socket file left is replaced by
        // the next server to listen there.
        let _ = std::fs::remove_file(&self.path);
    }
}
