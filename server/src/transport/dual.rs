//! \[DESIGN\] A stream in two handles (RULES.md D42): one the runtime
//! reads and writes, and one a thread reads and writes itself
//! (`Transport::direct`), so a worker takes a fast client's next request,
//! and answers it, without the runtime's threads (`native/lent.rs`). TCP
//! and Unix sockets have the two: a socket of the standard library's,
//! cloned, is the same socket in the same mode, so the thread's handle
//! waits, as `Direct::wait` says, only while the runtime leaves the stream
//! alone.

use std::io::{self, Read, Write};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use super::{Direct, Transport};

/// A socket of the standard library's, as `Dual` reads and writes it apart
/// from the runtime.
pub trait Socket: Read + Write + Send + 'static {
    /// Whether its reads and writes return at once with what there is, or
    /// wait for it.
    ///
    /// # Errors
    ///
    /// The system's, if the socket cannot be told.
    fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()>;
    /// How long a read or a write waits, when they do.
    ///
    /// # Errors
    ///
    /// The system's, if the socket cannot be told.
    fn set_timeouts(&self, within: Option<Duration>) -> io::Result<()>;
}

impl Socket for std::net::TcpStream {
    fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        std::net::TcpStream::set_nonblocking(self, nonblocking)
    }

    fn set_timeouts(&self, within: Option<Duration>) -> io::Result<()> {
        self.set_read_timeout(within)?;
        self.set_write_timeout(within)
    }
}

#[cfg(unix)]
impl Socket for std::os::unix::net::UnixStream {
    fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        std::os::unix::net::UnixStream::set_nonblocking(self, nonblocking)
    }

    fn set_timeouts(&self, within: Option<Duration>) -> io::Result<()> {
        self.set_read_timeout(within)?;
        self.set_write_timeout(within)
    }
}

/// \[DESIGN\] A stream in two handles: `io`, the runtime's, and `now`, a
/// thread's own (`Direct`). A wait is set on `now` in whole milliseconds,
/// the finest a socket's is, and only as it changes: a worker asking for
/// a fast client's next request asks for the same wait, less the
/// microseconds gone, so it sets nothing (`native/lent.rs`).
pub struct Dual<A, S> {
    io: A,
    now: S,
    /// Whether `now` waits for what it lacks (`Direct::wait`).
    waiting: bool,
    /// How long `now` is set to wait, when it does, if it was ever set.
    timeout: Option<Duration>,
}

impl<A, S> Dual<A, S> {
    /// `io` and `now`, two handles of one stream, `now` not waiting.
    pub fn new(io: A, now: S) -> Dual<A, S> {
        Dual { io, now, waiting: false, timeout: None }
    }
}

/// One millisecond: the finest a socket's wait is set in.
const MILLI: Duration = Duration::from_millis(1);

/// `within` rounded up to whole milliseconds.
fn whole_millis(within: Duration) -> Duration {
    let truncated = Duration::from_millis(u64::try_from(within.as_millis()).unwrap_or(u64::MAX));
    if truncated < within { truncated.saturating_add(MILLI) } else { truncated }
}

impl Dual<tokio::net::TcpStream, std::net::TcpStream> {
    /// `stream` in two handles.
    ///
    /// # Errors
    ///
    /// The system's, if the socket cannot be cloned, or registered with
    /// the runtime again.
    pub fn tcp(stream: tokio::net::TcpStream) -> io::Result<Self> {
        let io = stream.into_std()?;
        let now = io.try_clone()?;
        Ok(Dual::new(tokio::net::TcpStream::from_std(io)?, now))
    }
}

#[cfg(unix)]
impl Dual<tokio::net::UnixStream, std::os::unix::net::UnixStream> {
    /// `stream` in two handles.
    ///
    /// # Errors
    ///
    /// The system's, if the socket cannot be cloned, or registered with
    /// the runtime again.
    pub fn unix(stream: tokio::net::UnixStream) -> io::Result<Self> {
        let io = stream.into_std()?;
        let now = io.try_clone()?;
        Ok(Dual::new(tokio::net::UnixStream::from_std(io)?, now))
    }
}

impl<A: AsyncRead + Unpin, S: Unpin> AsyncRead for Dual<A, S> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_read(cx, buf)
    }
}

impl<A: AsyncWrite + Unpin, S: Unpin> AsyncWrite for Dual<A, S> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, buf)
    }

    fn poll_write_vectored(mut self: Pin<&mut Self>, cx: &mut Context<'_>, bufs: &[io::IoSlice<'_>]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.io).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.io.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}

impl<A: Send + 'static, S: Socket> Read for Dual<A, S> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.now.read(buf)
    }
}

impl<A: Send + 'static, S: Socket> Write for Dual<A, S> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.now.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.now.flush()
    }
}

impl<A: Send + 'static, S: Socket> Direct for Dual<A, S> {
    fn wait(&mut self, within: Option<Duration>) -> io::Result<()> {
        if let Some(wait) = within.map(whole_millis)
            && self.timeout != Some(wait)
        {
            self.now.set_timeouts(Some(wait))?;
            self.timeout = Some(wait);
        }
        if within.is_some() != self.waiting {
            self.now.set_nonblocking(within.is_none())?;
            self.waiting = within.is_some();
        }
        Ok(())
    }
}

impl<A: AsyncRead + AsyncWrite + Unpin + Send + 'static, S: Socket + Unpin> Transport for Dual<A, S> {
    fn direct(&mut self) -> Option<&mut dyn Direct> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use std::io::ErrorKind;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    /// A socket that notes what it is told, and reads and writes nothing.
    #[derive(Default)]
    struct Told(std::cell::RefCell<Vec<String>>);

    impl Read for Told {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Err(ErrorKind::WouldBlock.into())
        }
    }

    impl Write for Told {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Socket for Told {
        fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
            self.0.borrow_mut().push(format!("nonblocking {nonblocking}"));
            Ok(())
        }

        fn set_timeouts(&self, within: Option<Duration>) -> io::Result<()> {
            self.0.borrow_mut().push(format!("timeouts {:?}", within.map(|w| w.as_millis())));
            Ok(())
        }
    }

    /// Verifies: [DESIGN] RULES.md D42: a wait is set in whole
    /// milliseconds, rounded up, and only as it changes: the timeout when
    /// it differs from the one set, the blocking mode when a wait begins
    /// or ends. MC/DC pairs for setting the timeout, from a wait of a new
    /// length: none asked; the same length asked. For the mode, from a
    /// first wait: a wait again; none, twice.
    #[test]
    fn a_wait_is_set_in_whole_milliseconds_only_as_it_changes() {
        assert_eq!(whole_millis(Duration::from_millis(1)), Duration::from_millis(1));
        assert_eq!(whole_millis(Duration::from_millis(1) + Duration::from_nanos(1)), Duration::from_millis(2));
        assert_eq!(whole_millis(Duration::ZERO), Duration::ZERO);
        let mut dual = Dual::new((), Told::default());
        let told = |of: &mut Dual<(), Told>, within: Option<Duration>| {
            of.wait(within).unwrap();
            of.now.0.take()
        };
        assert_eq!(told(&mut dual, Some(Duration::from_micros(999))), ["timeouts Some(1)", "nonblocking false"], "a first wait");
        assert!(told(&mut dual, Some(Duration::from_micros(998))).is_empty(), "the same millisecond again");
        assert_eq!(told(&mut dual, Some(Duration::from_millis(2))), ["timeouts Some(2)"], "a longer wait, still waiting");
        assert_eq!(told(&mut dual, None), ["nonblocking true"], "the wait ended");
        assert!(told(&mut dual, None).is_empty(), "ended already");
        assert_eq!(told(&mut dual, Some(Duration::from_millis(2))), ["nonblocking false"], "waiting again, as long as last");
    }

    /// A TCP connection of loopback in two handles, and its client's end,
    /// on a runtime of its own.
    fn connected() -> (tokio::runtime::Runtime, Box<dyn Transport>, tokio::net::TcpStream) {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let (server, client) = runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let client = tokio::net::TcpStream::connect(listener.local_addr().unwrap());
            let (accepted, client) = tokio::join!(listener.accept(), client);
            (Dual::tcp(accepted.unwrap().0).unwrap(), client.unwrap())
        });
        (runtime, Box::new(server), client)
    }

    /// Verifies: [DESIGN] RULES.md D42: a TCP stream has a second handle
    /// a thread reads and writes itself: with nothing there, a read says
    /// so at once, or once the wait it was told runs out; what the client
    /// sends it reads, and what it writes the client reads; and the
    /// runtime's handle reads and writes the same stream once the wait is
    /// ended. The wait is not set again when it is the same.
    #[test]
    fn a_tcp_stream_is_read_and_written_by_a_thread_itself() {
        let (runtime, mut server, mut client) = connected();
        let mut buf = [0_u8; 8];
        let direct = server.direct().unwrap();
        assert_eq!(direct.read(&mut buf).unwrap_err().kind(), ErrorKind::WouldBlock, "nothing yet, no wait");
        direct.wait(Some(Duration::from_millis(1))).unwrap();
        direct.wait(Some(Duration::from_millis(1))).unwrap();
        let timed_out = direct.read(&mut buf).unwrap_err().kind();
        assert!(matches!(timed_out, ErrorKind::WouldBlock | ErrorKind::TimedOut), "waited, then {timed_out:?}");
        runtime.block_on(client.write_all(b"ab")).unwrap();
        assert_eq!(direct.read(&mut buf).unwrap(), 2);
        assert_eq!(&buf[..2], b"ab");
        assert_eq!(direct.write(b"cd").unwrap(), 2);
        direct.flush().unwrap();
        runtime.block_on(client.read_exact(&mut buf[..2])).unwrap();
        assert_eq!(&buf[..2], b"cd");
        direct.wait(None).unwrap();
        runtime.block_on(async {
            client.write_all(b"ef").await.unwrap();
            server.read_exact(&mut buf[..2]).await.unwrap();
            assert_eq!(&buf[..2], b"ef", "through the runtime");
            server.write_all(b"gh").await.unwrap();
            server.flush().await.unwrap();
            client.read_exact(&mut buf[..2]).await.unwrap();
            assert_eq!(&buf[..2], b"gh");
            assert_eq!(server.is_write_vectored(), client.is_write_vectored(), "as its stream");
            server.shutdown().await.unwrap();
            assert_eq!(client.read(&mut buf).await.unwrap(), 0, "closed");
        });
    }
}
