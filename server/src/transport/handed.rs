//! \[DESIGN\] A stream handed between the runtime and a thread (RULES.md
//! D42). The runtime has it, registered with it, until a worker waits on
//! it for a fast client's next request (`Direct::wait`). Then the thread
//! has it alone, the runtime's interest in it given up, so a request that
//! comes wakes no thread but the one waiting for it. The runtime has it
//! again as it next reads or writes it (`native/wire.rs`). TCP and Unix
//! sockets are handed so, as a socket of the standard library's registered
//! with the runtime and taken back from it, whole, in the mode of whoever
//! has it (`Socket`).

use std::io::{self, Read, Write};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::runtime::Handle;

use super::{Direct, Transport};

/// A socket of the standard library's, as a thread reads and writes it,
/// and as the runtime does once it is registered.
pub trait Socket: Read + Write + Send + Unpin + Sized + 'static {
    /// It, registered with the runtime, as the runtime reads and writes it.
    type Registered: AsyncRead + AsyncWrite + Unpin + Send + 'static;
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
    /// It, registered with the runtime of the task calling this; its
    /// reads and writes must return at once first.
    ///
    /// # Errors
    ///
    /// The system's, if it cannot be registered.
    fn register(self) -> io::Result<Self::Registered>;
    /// `registered`, taken back from the runtime.
    ///
    /// # Errors
    ///
    /// The system's, if it cannot be taken back.
    fn deregister(registered: Self::Registered) -> io::Result<Self>;
    /// What `registered` has to read now, into `buf`: `WouldBlock` for
    /// nothing.
    ///
    /// # Errors
    ///
    /// The read's.
    fn read_now(registered: &Self::Registered, buf: &mut [u8]) -> io::Result<usize>;
    /// How much of `buf` `registered` takes now: `WouldBlock` for nothing.
    ///
    /// # Errors
    ///
    /// The write's.
    fn write_now(registered: &Self::Registered, buf: &[u8]) -> io::Result<usize>;
}

impl Socket for std::net::TcpStream {
    type Registered = tokio::net::TcpStream;

    fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        std::net::TcpStream::set_nonblocking(self, nonblocking)
    }

    fn set_timeouts(&self, within: Option<Duration>) -> io::Result<()> {
        self.set_read_timeout(within)?;
        self.set_write_timeout(within)
    }

    fn register(self) -> io::Result<tokio::net::TcpStream> {
        tokio::net::TcpStream::from_std(self)
    }

    fn deregister(registered: tokio::net::TcpStream) -> io::Result<Self> {
        registered.into_std()
    }

    fn read_now(registered: &tokio::net::TcpStream, buf: &mut [u8]) -> io::Result<usize> {
        registered.try_read(buf)
    }

    fn write_now(registered: &tokio::net::TcpStream, buf: &[u8]) -> io::Result<usize> {
        registered.try_write(buf)
    }
}

#[cfg(unix)]
impl Socket for std::os::unix::net::UnixStream {
    type Registered = tokio::net::UnixStream;

    fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        std::os::unix::net::UnixStream::set_nonblocking(self, nonblocking)
    }

    fn set_timeouts(&self, within: Option<Duration>) -> io::Result<()> {
        self.set_read_timeout(within)?;
        self.set_write_timeout(within)
    }

    fn register(self) -> io::Result<tokio::net::UnixStream> {
        tokio::net::UnixStream::from_std(self)
    }

    fn deregister(registered: tokio::net::UnixStream) -> io::Result<Self> {
        registered.into_std()
    }

    fn read_now(registered: &tokio::net::UnixStream, buf: &mut [u8]) -> io::Result<usize> {
        registered.try_read(buf)
    }

    fn write_now(registered: &tokio::net::UnixStream, buf: &[u8]) -> io::Result<usize> {
        registered.try_write(buf)
    }
}

/// Who has the stream.
enum Held<S: Socket> {
    /// The runtime, it registered.
    Runtime(S::Registered),
    /// A thread, it waiting as long as `Handed::timeout` for what it lacks.
    Thread(S),
    /// No one: lost as it was handed over, so it is read and written no
    /// more.
    Lost,
}

/// \[DESIGN\] A stream handed between the runtime and a thread. A wait is
/// set in whole milliseconds, the finest a socket's is, and only as it
/// changes: a worker asking for a fast client's next request asks for the
/// same wait, less the microseconds gone, so it sets nothing
/// (`native/lent.rs`).
pub struct Handed<S: Socket> {
    held: Held<S>,
    /// How long the thread's reads and writes wait, if it was ever set.
    timeout: Option<Duration>,
    /// The runtime the stream is registered with, as it is handed back.
    runtime: Handle,
}

/// One millisecond: the finest a socket's wait is set in.
const MILLI: Duration = Duration::from_millis(1);

/// `within` rounded up to whole milliseconds.
fn whole_millis(within: Duration) -> Duration {
    let truncated = Duration::from_millis(u64::try_from(within.as_millis()).unwrap_or(u64::MAX));
    if truncated < within { truncated.saturating_add(MILLI) } else { truncated }
}

/// The stream was lost as it was handed over.
fn lost() -> io::Error {
    io::Error::new(io::ErrorKind::NotConnected, "[DESIGN] the stream was lost as it was handed between the runtime and a thread")
}

impl<S: Socket> Handed<S> {
    /// `registered`, the runtime's: the runtime of the task calling this.
    ///
    /// # Errors
    ///
    /// Called outside a runtime.
    pub fn new(registered: S::Registered) -> io::Result<Handed<S>> {
        let runtime = Handle::try_current().map_err(io::Error::other)?;
        Ok(Handed { held: Held::Runtime(registered), timeout: None, runtime })
    }

    /// Hands the stream to a thread, if the runtime has it: it waits from
    /// now, as long as `timeout` says.
    ///
    /// # Errors
    ///
    /// The system's: the stream is lost.
    fn hand_to_thread(&mut self) -> io::Result<()> {
        self.held = match std::mem::replace(&mut self.held, Held::Lost) {
            Held::Runtime(registered) => {
                let socket = S::deregister(registered)?;
                socket.set_nonblocking(false)?;
                Held::Thread(socket)
            }
            held @ (Held::Thread(_) | Held::Lost) => held,
        };
        Ok(())
    }

    /// Hands the stream to the runtime, if a thread has it.
    ///
    /// # Errors
    ///
    /// The system's: the stream is lost.
    fn hand_to_runtime(&mut self) -> io::Result<()> {
        self.held = match std::mem::replace(&mut self.held, Held::Lost) {
            Held::Thread(socket) => {
                socket.set_nonblocking(true)?;
                let _entered = self.runtime.enter();
                Held::Runtime(socket.register()?)
            }
            held @ (Held::Runtime(_) | Held::Lost) => held,
        };
        Ok(())
    }

    /// The stream as the runtime's, handed to it if a thread had it.
    ///
    /// # Errors
    ///
    /// The stream is lost.
    fn registered(&mut self) -> io::Result<&mut S::Registered> {
        self.hand_to_runtime()?;
        match &mut self.held {
            Held::Runtime(registered) => Ok(registered),
            Held::Thread(_) | Held::Lost => Err(lost()),
        }
    }
}

impl<S: Socket> AsyncRead for Handed<S> {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        Pin::new(self.get_mut().registered()?).poll_read(cx, buf)
    }
}

impl<S: Socket> AsyncWrite for Handed<S> {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(self.get_mut().registered()?).poll_write(cx, buf)
    }

    fn poll_write_vectored(self: Pin<&mut Self>, cx: &mut Context<'_>, bufs: &[io::IoSlice<'_>]) -> Poll<io::Result<usize>> {
        Pin::new(self.get_mut().registered()?).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        match &self.held {
            Held::Runtime(registered) => registered.is_write_vectored(),
            Held::Thread(_) | Held::Lost => false,
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(self.get_mut().registered()?).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(self.get_mut().registered()?).poll_shutdown(cx)
    }
}

impl<S: Socket> Read for Handed<S> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match &mut self.held {
            Held::Runtime(registered) => S::read_now(registered, buf),
            Held::Thread(socket) => socket.read(buf),
            Held::Lost => Err(lost()),
        }
    }
}

impl<S: Socket> Write for Handed<S> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match &mut self.held {
            Held::Runtime(registered) => S::write_now(registered, buf),
            Held::Thread(socket) => socket.write(buf),
            Held::Lost => Err(lost()),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match &mut self.held {
            // A socket the runtime has holds nothing back from a write.
            Held::Runtime(_) => Ok(()),
            Held::Thread(socket) => socket.flush(),
            Held::Lost => Err(lost()),
        }
    }
}

impl<S: Socket> Direct for Handed<S> {
    fn wait(&mut self, within: Duration) -> io::Result<()> {
        self.hand_to_thread()?;
        let Held::Thread(socket) = &self.held else { return Err(lost()) };
        let wait = whole_millis(within);
        if self.timeout != Some(wait) {
            socket.set_timeouts(Some(wait))?;
            self.timeout = Some(wait);
        }
        Ok(())
    }
}

impl<S: Socket> Transport for Handed<S> {
    fn direct(&mut self) -> Option<&mut dyn Direct> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::io::ErrorKind;
    use std::task::Waker;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    /// A socket that notes what it is told, reads nothing and takes every
    /// write, as a thread's and, registered, as the runtime's alike; lost
    /// as it is taken back from the runtime, if told to be.
    #[derive(Default)]
    struct Told {
        noted: RefCell<Vec<String>>,
        lose: Cell<bool>,
    }

    impl Told {
        fn note(&self, what: &str) {
            self.noted.borrow_mut().push(what.to_owned());
        }
    }

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

    impl AsyncRead for Told {
        fn poll_read(self: Pin<&mut Self>, _: &mut Context<'_>, _: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
            Poll::Pending
        }
    }

    impl AsyncWrite for Told {
        fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    impl Socket for Told {
        type Registered = Told;

        fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
            self.note(&format!("nonblocking {nonblocking}"));
            Ok(())
        }

        fn set_timeouts(&self, within: Option<Duration>) -> io::Result<()> {
            self.note(&format!("timeouts {:?}", within.map(|w| w.as_millis())));
            Ok(())
        }

        fn register(self) -> io::Result<Told> {
            self.note("registered");
            Ok(self)
        }

        fn deregister(registered: Told) -> io::Result<Told> {
            if registered.lose.get() {
                return Err(ErrorKind::Other.into());
            }
            registered.note("deregistered");
            Ok(registered)
        }

        fn read_now(registered: &Told, _buf: &mut [u8]) -> io::Result<usize> {
            registered.note("read now");
            Err(ErrorKind::WouldBlock.into())
        }

        fn write_now(registered: &Told, buf: &[u8]) -> io::Result<usize> {
            registered.note("written now");
            Ok(buf.len())
        }
    }

    /// What `handed`'s socket was told since last asked.
    fn told(handed: &Handed<Told>) -> Vec<String> {
        match &handed.held {
            Held::Runtime(socket) | Held::Thread(socket) => socket.noted.take(),
            Held::Lost => vec!["lost".to_owned()],
        }
    }

    /// `socket`, the runtime's, on a runtime of its own, entered.
    fn handed(socket: Told) -> (tokio::runtime::Runtime, Handed<Told>) {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let handed = {
            let _entered = runtime.enter();
            Handed::<Told>::new(socket).unwrap()
        };
        (runtime, handed)
    }

    /// Verifies: [DESIGN] RULES.md D42: a stream is read and written apart
    /// from the runtime while the runtime has it; handed to the thread that
    /// waits on it, taken back from the runtime and waiting from then; and
    /// handed back as the runtime reads or writes it, waiting no more.
    /// MC/DC pairs for who has it, from the runtime: a wait hands it over;
    /// a read apart from the runtime does not. From the thread: a poll
    /// hands it back; a wait does not.
    #[test]
    fn a_stream_is_handed_to_the_thread_that_waits_and_back_as_the_runtime_reads() {
        let (_runtime, mut handed) = handed(Told::default());
        let mut buf = [0_u8; 4];
        let direct: &mut dyn Direct = &mut handed;
        assert_eq!(direct.read(&mut buf).unwrap_err().kind(), ErrorKind::WouldBlock);
        assert_eq!(direct.write(b"ab").unwrap(), 2);
        direct.flush().unwrap();
        assert_eq!(told(&handed), ["read now", "written now"], "the runtime's, read and written apart from it");
        handed.wait(MILLI).unwrap();
        assert_eq!(told(&handed), ["deregistered", "nonblocking false", "timeouts Some(1)"], "handed to the thread, waiting");
        handed.wait(MILLI).unwrap();
        assert!(told(&handed).is_empty(), "the thread's still");
        assert!(!AsyncWrite::is_write_vectored(&handed), "not the runtime's to ask");
        let mut cx = Context::from_waker(Waker::noop());
        let mut read = ReadBuf::new(&mut buf);
        assert!(Pin::new(&mut handed).poll_read(&mut cx, &mut read).is_pending());
        assert_eq!(told(&handed), ["nonblocking true", "registered"], "handed back as the runtime reads");
        assert!(matches!(Pin::new(&mut handed).poll_write(&mut cx, b"cd"), Poll::Ready(Ok(2))));
        assert!(matches!(Pin::new(&mut handed).poll_shutdown(&mut cx), Poll::Ready(Ok(()))));
        assert!(told(&handed).is_empty(), "the runtime's already");
        handed.wait(MILLI).unwrap();
        assert_eq!(told(&handed), ["deregistered", "nonblocking false"], "the thread's again, waiting as long as last");
    }

    /// Verifies: [DESIGN] RULES.md D42: a wait is set in whole
    /// milliseconds, rounded up, and only as it changes. MC/DC pairs, from
    /// a wait of a new length: the same length asked; a longer one.
    #[test]
    fn a_wait_is_set_in_whole_milliseconds_only_as_it_changes() {
        assert_eq!(whole_millis(MILLI), MILLI);
        assert_eq!(whole_millis(MILLI + Duration::from_nanos(1)), Duration::from_millis(2));
        assert_eq!(whole_millis(Duration::ZERO), Duration::ZERO);
        let (_runtime, mut handed) = handed(Told::default());
        handed.wait(Duration::from_micros(999)).unwrap();
        assert_eq!(told(&handed), ["deregistered", "nonblocking false", "timeouts Some(1)"], "a first wait");
        handed.wait(Duration::from_micros(998)).unwrap();
        assert!(told(&handed).is_empty(), "the same millisecond again");
        handed.wait(Duration::from_millis(2)).unwrap();
        assert_eq!(told(&handed), ["timeouts Some(2)"], "a longer wait");
    }

    /// Verifies: [DESIGN] RULES.md D42: a stream lost as it is handed over
    /// fails the hand-over, and is read and written no more, by the thread
    /// or the runtime, each failing rather than reaching it.
    #[test]
    fn a_stream_lost_as_it_is_handed_over_is_read_no_more() {
        let socket = Told::default();
        socket.lose.set(true);
        let (_runtime, mut handed) = handed(socket);
        assert!(handed.wait(Duration::from_millis(1)).is_err(), "lost taking it back");
        assert_eq!(told(&handed), ["lost"]);
        let mut buf = [0_u8; 4];
        let direct: &mut dyn Direct = &mut handed;
        assert_eq!(direct.read(&mut buf).unwrap_err().kind(), ErrorKind::NotConnected);
        assert_eq!(direct.write(b"ab").unwrap_err().kind(), ErrorKind::NotConnected);
        assert_eq!(direct.flush().unwrap_err().kind(), ErrorKind::NotConnected);
        assert!(direct.wait(Duration::from_millis(1)).is_err(), "lost still");
        let mut cx = Context::from_waker(Waker::noop());
        let mut read = ReadBuf::new(&mut buf);
        assert!(matches!(Pin::new(&mut handed).poll_read(&mut cx, &mut read), Poll::Ready(Err(e)) if e.kind() == ErrorKind::NotConnected));
        assert!(matches!(Pin::new(&mut handed).poll_write(&mut cx, b"cd"), Poll::Ready(Err(_))));
        assert!(matches!(Pin::new(&mut handed).poll_shutdown(&mut cx), Poll::Ready(Err(_))));
        assert!(!AsyncWrite::is_write_vectored(&handed));
    }

    /// A TCP connection of loopback the runtime has, and its client's end,
    /// on a runtime of its own.
    fn connected() -> (tokio::runtime::Runtime, Box<dyn Transport>, tokio::net::TcpStream) {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let (server, client) = runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let client = tokio::net::TcpStream::connect(listener.local_addr().unwrap());
            let (accepted, client) = tokio::join!(listener.accept(), client);
            (Handed::<std::net::TcpStream>::new(accepted.unwrap().0).unwrap(), client.unwrap())
        });
        (runtime, Box::new(server), client)
    }

    /// Verifies: [DESIGN] RULES.md D42: a TCP stream is handed to a thread
    /// that reads and writes it itself: with nothing there, a read says so
    /// at once, or once the wait it was told runs out; what the client
    /// sends it reads, and what it writes the client reads; the runtime
    /// reads and writes the same stream once it is handed back, and a
    /// thread writes it apart from the runtime meanwhile, without taking
    /// it. The wait is not set again when it is the same.
    #[test]
    fn a_tcp_stream_is_read_and_written_by_a_thread_itself() {
        let (runtime, mut server, mut client) = connected();
        let mut buf = [0_u8; 8];
        let direct = server.direct().unwrap();
        assert_eq!(direct.read(&mut buf).unwrap_err().kind(), ErrorKind::WouldBlock, "nothing yet, no wait");
        direct.wait(Duration::from_millis(1)).unwrap();
        direct.wait(Duration::from_millis(1)).unwrap();
        let timed_out = direct.read(&mut buf).unwrap_err().kind();
        assert!(matches!(timed_out, ErrorKind::WouldBlock | ErrorKind::TimedOut), "waited, then {timed_out:?}");
        runtime.block_on(client.write_all(b"ab")).unwrap();
        assert_eq!(direct.read(&mut buf).unwrap(), 2);
        assert_eq!(&buf[..2], b"ab");
        assert_eq!(direct.write(b"cd").unwrap(), 2);
        direct.flush().unwrap();
        runtime.block_on(client.read_exact(&mut buf[..2])).unwrap();
        assert_eq!(&buf[..2], b"cd");
        runtime.block_on(async {
            client.write_all(b"ef").await.unwrap();
            server.read_exact(&mut buf[..2]).await.unwrap();
            assert_eq!(&buf[..2], b"ef", "through the runtime, handed back");
            server.write_all(b"gh").await.unwrap();
            server.flush().await.unwrap();
            client.read_exact(&mut buf[..2]).await.unwrap();
            assert_eq!(&buf[..2], b"gh");
            assert_eq!(server.is_write_vectored(), client.is_write_vectored(), "as its stream");
            assert_eq!(server.direct().unwrap().write(b"ij").unwrap(), 2, "apart from the runtime, the runtime's still");
            client.read_exact(&mut buf[..2]).await.unwrap();
            assert_eq!(&buf[..2], b"ij");
            server.shutdown().await.unwrap();
            assert_eq!(client.read(&mut buf).await.unwrap(), 0, "closed");
        });
    }
}
