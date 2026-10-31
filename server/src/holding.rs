//! \[DESIGN\] What a connection holds while it is served (RULES.md D42):
//! its place under SERVER.MAX_CONNECTIONS, or among the refusals in
//! flight, and its row of INFORMATION_SCHEMA.CONNECTIONS. Its stream gives
//! them back the moment it shuts down or is dropped, before the system
//! closes the socket, so a client that has seen its connection closed is
//! never counted, nor shown. Over TLS they are given back before
//! close_notify is sent, so a client that does not take it keeps its
//! socket open outside the limit, for SERVER.IDLE_TIMEOUT_MS at most,
//! until the server hurries if that is 0.

use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::transport::Transport;

/// What a connection holds; none once given back.
type Kept = Option<Vec<Box<dyn Send>>>;

/// \[DESIGN\] What a connection holds, shared with its stream.
#[derive(Clone)]
pub(crate) struct Hold(Arc<Mutex<Kept>>);

impl Hold {
    /// A hold of nothing yet.
    pub(crate) fn new() -> Hold {
        Hold(Arc::new(Mutex::new(Some(Vec::new()))))
    }

    /// Holds `what` until the stream closes; given back at once if it has.
    pub(crate) fn keep(&self, what: impl Send + 'static) {
        let mut kept = self.locked();
        if let Some(held) = kept.as_mut() {
            held.push(Box::new(what));
        }
    }

    /// Gives back all it holds, and anything kept from now.
    fn release(&self) {
        // Dropped outside the lock: a place given back wakes its waiters.
        let held = self.locked().take();
        drop(held);
    }

    fn locked(&self) -> std::sync::MutexGuard<'_, Kept> {
        // Nothing panics while it is locked, so a poisoned lock is whole.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// \[DESIGN\] A connection's stream, `inner`, that gives back `hold` as it
/// shuts down or is dropped, before `inner` closes.
pub(crate) struct Holding {
    hold: Hold,
    inner: Box<dyn Transport>,
}

impl Holding {
    /// `inner`, giving back `hold` as it closes.
    pub(crate) fn new(inner: Box<dyn Transport>, hold: Hold) -> Holding {
        Holding { hold, inner }
    }
}

impl Drop for Holding {
    fn drop(&mut self) {
        self.hold.release();
    }
}

impl AsyncRead for Holding {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for Holding {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_write_vectored(mut self: Pin<&mut Self>, cx: &mut Context<'_>, bufs: &[io::IoSlice<'_>]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.hold.release();
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    /// Counts its drops in its counter.
    struct Counted(Arc<AtomicUsize>);

    impl Drop for Counted {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Verifies: [DESIGN] RULES.md D42: a stream gives back what its
    /// connection holds before its peer reads its end, as it shuts down,
    /// and as it is dropped; what is kept after is given back at once; it
    /// reads and writes through.
    #[test]
    fn a_stream_gives_back_its_hold_before_it_closes() {
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        runtime.block_on(async {
            let given = Arc::new(AtomicUsize::new(0));
            let (near, mut far) = tokio::io::duplex(64);
            let hold = Hold::new();
            hold.keep(Counted(Arc::clone(&given)));
            let mut holding = Holding::new(Box::new(near), hold.clone());
            holding.write_all(b"a").await.unwrap();
            assert_eq!(holding.write_vectored(&[io::IoSlice::new(b"b")]).await.unwrap(), 1);
            holding.flush().await.unwrap();
            assert_eq!(holding.is_write_vectored(), tokio::io::duplex(1).0.is_write_vectored(), "as its inner stream");
            let mut read = [0_u8; 2];
            far.read_exact(&mut read).await.unwrap();
            assert_eq!(&read, b"ab");
            far.write_all(b"c").await.unwrap();
            holding.read_exact(&mut read[..1]).await.unwrap();
            assert_eq!(given.load(Ordering::SeqCst), 0, "held while open");
            holding.shutdown().await.unwrap();
            assert_eq!(given.load(Ordering::SeqCst), 1, "given back as it shut down");
            hold.keep(Counted(Arc::clone(&given)));
            assert_eq!(given.load(Ordering::SeqCst), 2, "kept after: given back at once");
            let dropped = Hold::new();
            dropped.keep(Counted(Arc::clone(&given)));
            drop(Holding::new(Box::new(tokio::io::duplex(1).0), dropped));
            assert_eq!(given.load(Ordering::SeqCst), 3, "given back as it is dropped");
        });
    }
}
