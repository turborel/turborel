//! Unit test of a wire a thread reads and writes itself over a stream
//! whose calls a signal interrupts (RULES.md D1047, R-TEST-1).

use std::collections::VecDeque;
use std::io::{self, ErrorKind, Read, Write};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use super::*;
use crate::transport::Direct;

/// A stream in memory a thread reads and writes itself, each read, write
/// and flush of which a signal interrupts once before it is done.
struct Interrupting {
    from: VecDeque<u8>,
    to: Arc<Mutex<Vec<u8>>>,
    /// The flushes done.
    flushed: Arc<AtomicUsize>,
    next: bool,
}

impl Interrupting {
    /// Fails as a call a signal interrupts does, every other time.
    fn interrupt(&mut self) -> io::Result<()> {
        self.next = !self.next;
        if self.next { Err(io::Error::from(ErrorKind::Interrupted)) } else { Ok(()) }
    }
}

impl Read for Interrupting {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.interrupt()?;
        if self.from.is_empty() {
            return Err(io::Error::from(ErrorKind::WouldBlock));
        }
        let n = buf.len().min(self.from.len());
        for (to, byte) in buf.iter_mut().zip(self.from.drain(..n)) {
            *to = byte;
        }
        Ok(n)
    }
}

impl Write for Interrupting {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.interrupt()?;
        self.to.lock().map_err(|_| io::Error::other("a test panicked"))?.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.interrupt()?;
        self.flushed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

impl Direct for Interrupting {
    fn wait(&mut self, _: Duration) -> io::Result<()> {
        Ok(())
    }
}

/// The runtime reads and writes none of it.
fn not_the_runtimes<T>() -> Poll<io::Result<T>> {
    Poll::Ready(Err(io::Error::from(ErrorKind::Unsupported)))
}

impl AsyncRead for Interrupting {
    fn poll_read(self: Pin<&mut Self>, _: &mut Context<'_>, _: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        not_the_runtimes()
    }
}

impl AsyncWrite for Interrupting {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, _: &[u8]) -> Poll<io::Result<usize>> {
        not_the_runtimes()
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        not_the_runtimes()
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        not_the_runtimes()
    }
}

impl Transport for Interrupting {
    fn direct(&mut self) -> Option<&mut dyn Direct> {
        Some(self)
    }
}

/// Verifies: [DESIGN] RULES.md D1047: a read, a write or a flush a
/// signal interrupts is made again, and is no failure of the connection.
/// A wire over a stream whose every call is interrupted once takes the
/// request sent and sends its reply whole, flushed once.
#[test]
fn a_read_a_write_or_a_flush_interrupted_is_made_again() {
    let (written, flushed) = (Arc::new(Mutex::new(Vec::new())), Arc::new(AtomicUsize::new(0)));
    let mut request = Vec::new();
    Request::Ping.encode(&mut request);
    let stream = Interrupting { from: request.into(), to: Arc::clone(&written), flushed: Arc::clone(&flushed), next: false };
    let mut wire = Wire::new(Box::new(stream), 1024, TypeRegistry::default());
    assert!(wire.fill_within(Duration::from_millis(1)).unwrap(), "read, though interrupted");
    assert!(matches!(wire.arrived(|came| matches!(came, Request::Ping).then_some(())), Arrived::Taken(())));
    wire.queue(&[Reply::Pong]);
    assert!(wire.sent_now().unwrap(), "written and flushed, though interrupted");
    let mut reply = Vec::new();
    Reply::Pong.encode(&mut reply);
    assert_eq!((written.lock().unwrap().clone(), flushed.load(Ordering::SeqCst)), (reply, 1));
}
