//! \[DESIGN\] A stream TLS secures, read and written by a thread itself
//! (RULES.md D42, D230). The session's records are taken from and given
//! to the socket below it as `Direct` reads and writes, so a worker takes
//! a fast client's next request over TLS as it does in the clear, and a
//! request answered wakes no thread but the one that runs it. The runtime
//! reads and writes the same session through `tokio_rustls` when no
//! worker has it. Both work on the one session, so no record is lost or
//! read twice between them.
//!
//! What the session holds to send is pushed to the socket before more
//! plaintext is taken, so it holds no more than one write's records
//! beside rustls's own limit. A record the socket cannot take now waits
//! in the session for the next flush, by the thread or the runtime. A
//! close_notify read ends the stream as a read of 0 bytes, as a socket
//! closed does. A socket closed without one fails the read
//! (`UnexpectedEof`), so a request cut short is never taken for a whole
//! one. rustls never renegotiates, so no read here waits on a handshake.

use std::io::{self, ErrorKind, IoSlice, Read, Write};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use rustls::ServerConnection;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use super::Prefixed;
use crate::transport::{Direct, Transport};

/// \[DESIGN\] A stream TLS secures, its socket one a thread may read and
/// write itself.
pub(super) struct Encrypted(pub(super) tokio_rustls::server::TlsStream<Prefixed>);

/// `e`, a failure of the session's, as an I/O error.
fn invalid(e: rustls::Error) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, e)
}

/// The socket is not one a thread reads or writes itself now: nothing
/// there, for the runtime to read.
fn not_direct() -> io::Error {
    io::Error::from(ErrorKind::WouldBlock)
}

/// The socket below the session, as a thread reads and writes it.
struct Below<'a>(&'a mut dyn Direct);

impl Read for Below<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl Write for Below<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.0.write_vectored(bufs)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

/// Sends the records `session` holds to `below`: `WouldBlock` if the
/// socket takes no more now, the rest kept in the session.
fn pushed(session: &mut ServerConnection, below: &mut dyn Direct) -> io::Result<()> {
    while session.wants_write() {
        if session.write_tls(&mut Below(&mut *below))? == 0 {
            return Err(ErrorKind::WriteZero.into());
        }
    }
    Ok(())
}

/// `pushed`, a socket full now taken as no failure: the records wait in
/// the session for the next flush.
fn pushed_if_room(session: &mut ServerConnection, below: &mut dyn Direct) -> io::Result<()> {
    let Err(e) = pushed(session, below) else { return Ok(()) };
    if e.kind() == ErrorKind::WouldBlock { Ok(()) } else { Err(e) }
}

/// The plaintext `session` has for `buf`: `None` if it has none yet and
/// the connection is open.
fn decrypted(session: &mut ServerConnection, buf: &mut [u8]) -> io::Result<Option<usize>> {
    match session.reader().read(buf) {
        Ok(n) => Ok(Some(n)),
        Err(e) => {
            if e.kind() == ErrorKind::WouldBlock {
                return Ok(None);
            }
            Err(e)
        }
    }
}

impl Read for Encrypted {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let (io, session) = self.0.get_mut();
        loop {
            if let Some(n) = decrypted(session, buf)? {
                return Ok(n);
            }
            let below = io.direct().ok_or_else(not_direct)?;
            let read = session.read_tls(&mut Below(&mut *below))?;
            session.process_new_packets().map_err(invalid)?;
            // An alert or a key update the session answers goes now, as
            // the runtime's reads send it.
            pushed_if_room(session, below)?;
            if read == 0 {
                // The socket closed: a close_notify read first ends the
                // stream cleanly, none fails it.
                return session.reader().read(buf);
            }
        }
    }
}

impl Write for Encrypted {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let (io, session) = self.0.get_mut();
        let below = io.direct().ok_or_else(not_direct)?;
        pushed(session, below)?;
        let taken = session.writer().write(buf)?;
        if taken == 0 {
            return Err(not_direct());
        }
        pushed_if_room(session, below)?;
        Ok(taken)
    }

    fn flush(&mut self) -> io::Result<()> {
        let (io, session) = self.0.get_mut();
        let below = io.direct().ok_or_else(not_direct)?;
        session.writer().flush()?;
        pushed(session, below)?;
        below.flush()
    }
}

impl Direct for Encrypted {
    fn wait(&mut self, within: Duration) -> io::Result<()> {
        self.0.get_mut().0.direct().ok_or_else(not_direct)?.wait(within)
    }
}

impl Transport for Encrypted {
    fn direct(&mut self) -> Option<&mut dyn Direct> {
        // A thread reads it itself only while its socket allows.
        self.0.get_mut().0.direct()?;
        Some(self)
    }

    fn watch(&self) -> Option<crate::transport::Watch> {
        self.0.get_ref().0.watch()
    }
}

impl AsyncRead for Encrypted {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl AsyncWrite for Encrypted {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_write_vectored(mut self: Pin<&mut Self>, cx: &mut Context<'_>, bufs: &[IoSlice<'_>]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.0.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use std::net::Shutdown;

    use rustls::pki_types::ServerName;
    use rustls::{ClientConnection, StreamOwned};
    use tokio_rustls::TlsAcceptor;

    use super::*;
    use crate::transport::Handed;

    /// A wait long enough for loopback to bring what was sent.
    const BROUGHT: Duration = Duration::from_secs(10);

    /// A TLS connection of loopback, its server's end secured on a runtime
    /// of its own, and its client's end.
    fn secured(test: &str) -> (tokio::runtime::Runtime, Encrypted, StreamOwned<ClientConnection, std::net::TcpStream>) {
        let pki = turborel_test_support::tls::pki(test);
        let (chain, key) = super::super::identity(&pki.chain.display().to_string(), &pki.key.display().to_string()).unwrap();
        let config = super::super::server_config(chain, key, None).unwrap();
        let client = turborel_client::tls::trusting(&pki.authority).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let listener = runtime.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
        let address = listener.local_addr().unwrap();
        let shaking = std::thread::spawn(move || {
            let tcp = std::net::TcpStream::connect(address).unwrap();
            let connection = ClientConnection::new(client, ServerName::try_from("localhost").unwrap()).unwrap();
            let mut stream = StreamOwned::new(connection, tcp);
            stream.conn.complete_io(&mut stream.sock).unwrap();
            stream
        });
        let server = runtime.block_on(async {
            let (accepted, _) = listener.accept().await.unwrap();
            let inner: Box<dyn Transport> = Box::new(Handed::<std::net::TcpStream>::new(accepted).unwrap());
            TlsAcceptor::from(config).accept(Prefixed { first: None, inner }).await.unwrap()
        });
        (runtime, Encrypted(server), shaking.join().unwrap())
    }

    /// What `direct` reads, `n` bytes, waiting for them.
    fn read_n(direct: &mut dyn Direct, n: usize) -> Vec<u8> {
        direct.wait(BROUGHT).unwrap();
        let mut got = vec![0; n];
        let mut at = 0;
        while at < n {
            at = at.saturating_add(direct.read(&mut got[at..]).unwrap());
        }
        got
    }

    /// Verifies: [DESIGN] RULES.md D230: a stream TLS secures is read and
    /// written by a thread itself: nothing there says so at once; what the
    /// client sends is read, the records decrypted; what is written the
    /// client reads, once flushed; a key update the client asks for is
    /// answered and the stream goes on; a close_notify ends it as a read of
    /// nothing.
    #[test]
    fn a_tls_stream_is_read_and_written_by_a_thread_itself() {
        let (_runtime, mut server, mut client) = secured("tls-direct");
        let direct = server.direct().unwrap();
        let nothing = direct.read(&mut [0_u8; 4]).unwrap_err().kind();
        assert_eq!(nothing, ErrorKind::WouldBlock, "nothing yet");
        client.write_all(b"hello").unwrap();
        assert_eq!(read_n(direct, 5), b"hello");
        let long = vec![7_u8; 100_000];
        let mut sent = 0;
        while sent < long.len() {
            sent = sent.saturating_add(direct.write(&long[sent..]).unwrap());
        }
        direct.flush().unwrap();
        let mut back = vec![0_u8; long.len()];
        client.read_exact(&mut back).unwrap();
        assert_eq!(back, long, "many records, read whole");
        client.conn.refresh_traffic_keys().unwrap();
        client.write_all(b"again").unwrap();
        assert_eq!(read_n(direct, 5), b"again", "under the new keys");
        client.conn.send_close_notify();
        client.flush().unwrap();
        direct.wait(BROUGHT).unwrap();
        assert_eq!(direct.read(&mut [0_u8; 4]).unwrap(), 0, "ended by close_notify");
    }

    /// Verifies: [DESIGN] RULES.md D230: a TLS client whose socket closes
    /// without close_notify fails the read, never ends it as a client that
    /// ended its stream would (MC/DC pair with the close_notify above).
    #[test]
    fn a_socket_closed_without_close_notify_fails_the_read() {
        let (_runtime, mut server, client) = secured("tls-direct-cut");
        client.sock.shutdown(Shutdown::Both).unwrap();
        let direct = server.direct().unwrap();
        direct.wait(BROUGHT).unwrap();
        assert_eq!(direct.read(&mut [0_u8; 4]).unwrap_err().kind(), ErrorKind::UnexpectedEof);
    }
}
