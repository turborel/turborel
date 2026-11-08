//! \[DESIGN\] Transports (RULES.md D42): a `Listener` plug-in accepts
//! connections, each a stream that reads and writes, with the peer it came
//! from. TCP, Unix sockets on Unix and named pipes on Windows are built
//! in; TLS (`tls.rs`) secures a listener's streams, in the connection's
//! own task. A TCP or Unix stream is handed to the worker answering a
//! request (`handed.rs`), so it takes a fast client's next itself, apart
//! from the runtime; a named pipe's, or a stream TLS secures, the runtime
//! reads for it.

mod handed;

use std::future::Future;
use std::io::{self, Read, Write};
use std::net::SocketAddr;
use std::pin::Pin;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};
use turborel::db::Database;

pub use handed::{Handed, Socket};

use crate::auth::Peer;
use crate::keepalive::Keepalive;

/// \[DESIGN\] A connection's stream: an extension point (RULES.md D42),
/// what a `Listener` accepts.
///
/// Contract (RULES.md R-SAFE-3): it reads and writes through the runtime.
/// One a thread can read and write itself, apart from the runtime, gives
/// itself so from `direct` (`Handed`); one that cannot, as a stream TLS
/// secures, leaves it `None`, the default, so an `impl Transport for It
/// {}` is enough.
pub trait Transport: AsyncRead + AsyncWrite + Unpin + Send + 'static {
    /// The stream as a thread reads and writes it itself, if it can.
    fn direct(&mut self) -> Option<&mut dyn Direct> {
        None
    }
}

/// \[DESIGN\] A stream a thread reads and writes itself, apart from the
/// runtime: a worker takes a fast client's next request so, and answers
/// it, waking no other thread (`native/lent.rs`).
///
/// Contract (RULES.md R-SAFE-3): `read` and `write` return at once with
/// what there is, `WouldBlock` for nothing, until `wait` says how long
/// they wait for it, `WouldBlock` or `TimedOut` once that runs out; a
/// read of 0 bytes is the client closing. While they wait, the stream is
/// the thread's, the runtime's interest in it given up, until the runtime
/// next reads or writes it, which takes it back and ends the wait. The
/// system may round the wait up to its timer's tick.
pub trait Direct: Read + Write {
    /// Makes `read` and `write` wait `within` for what they lack, until
    /// the runtime next reads or writes.
    ///
    /// # Errors
    ///
    /// The system's, if the stream cannot be told.
    fn wait(&mut self, within: Duration) -> io::Result<()>;
}

#[cfg(windows)]
impl Transport for tokio::net::windows::named_pipe::NamedPipeServer {}

#[cfg(test)]
impl Transport for tokio::io::DuplexStream {}

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

/// \[DESIGN\] The built-in TCP listener. Each connection it takes has
/// TCP keepalive on (`keepalive.rs`), at the times the settings of the
/// database it is given say (`Tcp::kept_alive_by`), or the system's own.
pub struct Tcp {
    listener: tokio::net::TcpListener,
    /// The database whose settings give keepalive's times, if any.
    db: Option<Database>,
}

impl Tcp {
    /// Listens at `address`.
    ///
    /// # Errors
    ///
    /// The system's, if it cannot.
    pub async fn bind(address: SocketAddr) -> io::Result<Tcp> {
        Ok(Tcp { listener: tokio::net::TcpListener::bind(address).await?, db: None })
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

    /// It, keepalive's times of each connection taken read from `db`'s
    /// settings as it is taken (SERVER.KEEPALIVE_IDLE_MS,
    /// SERVER.KEEPALIVE_INTERVAL_MS, SERVER.KEEPALIVE_PROBES).
    #[must_use]
    pub fn kept_alive_by(self, db: Database) -> Tcp {
        Tcp { db: Some(db), ..self }
    }

    /// The connection `stream` taken, set up: replies are flushed whole, so
    /// nothing waits for more to send; keepalive on, at its times.
    ///
    /// # Errors
    ///
    /// The system's, if it cannot; as `Keepalive::apply`.
    fn set_up(&self, stream: &tokio::net::TcpStream) -> io::Result<()> {
        stream.set_nodelay(true)?;
        self.db.as_ref().map(Keepalive::of).unwrap_or_default().apply(stream)
    }
}

impl Listener for Tcp {
    fn accept(&self) -> Accepting<'_> {
        Box::pin(async {
            let (accepted, address) = self.listener.accept().await?;
            self.set_up(&accepted)?;
            let stream: Box<dyn Transport> = Box::new(Handed::<std::net::TcpStream>::new(accepted)?);
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
    /// Its socket file, shown while it listens (RULES.md D56).
    noted: Option<turborel::files::Noted>,
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
        Ok(Local { listener: tokio::net::UnixListener::bind(path)?, path: path.to_path_buf(), noted: None })
    }

    /// It, its socket file shown as `noted` says while it listens.
    #[must_use]
    pub fn noting(mut self, noted: turborel::files::Noted) -> Local {
        self.noted = Some(noted);
        self
    }
}

#[cfg(unix)]
impl Listener for Local {
    fn accept(&self) -> Accepting<'_> {
        Box::pin(async {
            let (stream, _) = self.listener.accept().await?;
            let stream: Box<dyn Transport> = Box::new(Handed::<std::os::unix::net::UnixStream>::new(stream)?);
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

/// \[DESIGN\] The built-in named pipe listener, on Windows: the local
/// transport there, as a Unix socket is on Unix (RULES.md D42). Its first
/// instance is made as the first of its name, so a server cannot listen at
/// a name another process holds, nor another at one it holds; an instance
/// always waits, the next made before a connected one is handed over, so
/// the name is never free for another to take while it listens. Clients
/// of other machines are refused, so a client is of this machine
/// (`Peer::local`). Who of this machine may connect is as the system's
/// default security for a pipe says: its creator, the administrators and
/// the system fully; others may open it to read only, so they can send
/// nothing, Hello neither, and are closed when SERVER.LOGIN_TIMEOUT_MS runs
/// out. A pipe is gone once its last instance closes: there is no file to
/// remove.
#[cfg(windows)]
pub struct Pipe {
    /// The instance waiting for the next client.
    waiting: tokio::sync::Mutex<tokio::net::windows::named_pipe::NamedPipeServer>,
    /// The pipe's name, as `\\.\pipe\turborel`.
    name: String,
    /// Its name, shown while it listens (RULES.md D56).
    noted: Option<turborel::files::Noted>,
}

#[cfg(windows)]
impl Pipe {
    /// Listens at the pipe `name`; called within a tokio runtime.
    ///
    /// # Errors
    ///
    /// The system's; `AddrInUse` if a process holds a pipe of the name.
    pub fn bind(name: &str) -> io::Result<Pipe> {
        let first = pipe_instance(name, true).map_err(|e| {
            if e.kind() == io::ErrorKind::PermissionDenied {
                return io::Error::new(io::ErrorKind::AddrInUse, format!("{name} is in use ({e})"));
            }
            e
        })?;
        Ok(Pipe { waiting: tokio::sync::Mutex::new(first), name: name.to_owned(), noted: None })
    }

    /// It, its name shown as `noted` says while it listens.
    #[must_use]
    pub fn noting(mut self, noted: turborel::files::Noted) -> Pipe {
        self.noted = Some(noted);
        self
    }
}

/// An instance of the pipe `name`, the first of its name if `first`, and
/// then refused if the name is held already; clients of other machines
/// refused.
///
/// # Errors
///
/// The system's.
#[cfg(windows)]
fn pipe_instance(name: &str, first: bool) -> io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
    tokio::net::windows::named_pipe::ServerOptions::new().first_pipe_instance(first).reject_remote_clients(true).create(name)
}

#[cfg(windows)]
impl Listener for Pipe {
    fn accept(&self) -> Accepting<'_> {
        Box::pin(async {
            let mut waiting = self.waiting.lock().await;
            let connected = waiting.connect().await;
            // The next instance waits before this one is handed over, or
            // dropped if it failed: the name is never without one.
            let taken = std::mem::replace(&mut *waiting, pipe_instance(&self.name, false)?);
            connected?;
            let stream: Box<dyn Transport> = Box::new(taken);
            Ok((stream, Peer::local()))
        })
    }

    fn address(&self) -> String {
        self.name.clone()
    }
}

#[cfg(test)]
mod tests;
