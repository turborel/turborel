//! \[DESIGN\] A watch on a connection's peer (RULES.md D335). It is a copy
//! of its socket, never read, only looked at, so the connection's task finds
//! a client gone while a worker runs its statement, the stream lent to
//! the worker (`native.rs`), and has it cancelled (RULES.md D44).
//!
//! A look peeks at the socket's next byte, taking nothing. None, as the
//! client closed its end, or an error, as a connection reset, is the
//! client gone. A byte is a request sent ahead, which tells nothing, so
//! it is looked at again; and so is the end of the stream while the
//! connection's inbox holds what the client sent ahead, already read from
//! the socket, as a client that shut its side after its requests hears
//! each reply (RULES.md D234). The copy shares the socket's mode, so a look
//! waits as long as a read of the worker's would, a moment at most, so it
//! is made on a thread of the blocking pool, not the runtime's.
//!
//! A client that sent a request ahead and then closed is found only as
//! the worker reads that request; one behind TLS that closed with its
//! closing alert is found so too, the alert a byte to read.

use std::io::ErrorKind;
use std::mem::MaybeUninit;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// A watch on a connection's peer.
#[derive(Clone)]
pub struct Watch {
    socket: Arc<socket2::Socket>,
    /// Whether the connection's inbox holds what its client sent ahead,
    /// if the connection tells.
    ahead: Option<Arc<AtomicBool>>,
}

impl Watch {
    /// A watch through `copy`, a copy of the connection's socket.
    pub(crate) fn of(copy: socket2::Socket) -> Watch {
        Watch { socket: Arc::new(copy), ahead: None }
    }

    /// It, told by `ahead` whether the connection's inbox holds what its
    /// client sent ahead.
    pub(crate) fn told(self, ahead: Arc<AtomicBool>) -> Watch {
        Watch { ahead: Some(ahead), ..self }
    }

    /// Whether the peer is gone, as a look at the socket tells: the end of
    /// its stream, nothing sent ahead held, or an error.
    fn gone(&self) -> bool {
        let mut next = [MaybeUninit::uninit()];
        match self.socket.peek(&mut next) {
            Ok(read) => read == 0 && !self.ahead.as_ref().is_some_and(|a| a.load(Ordering::Acquire)),
            Err(e) => !matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted),
        }
    }

    /// Done once the peer is gone, looked at every `every`, the first
    /// look after the first `every`.
    pub(crate) async fn gone_within(self, every: Duration) {
        loop {
            tokio::time::sleep(every).await;
            let watch = self.clone();
            // A look whose thread was lost tells the peer is gone no more
            // than it tells it is not, so it is looked at again.
            if tokio::task::spawn_blocking(move || watch.gone()).await.unwrap_or(false) {
                return;
            }
        }
    }
}
