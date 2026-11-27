//! \[DESIGN\] What Ctrl-C does (RULES.md D46). Once connected, it cancels
//! the statement the session runs (RULES.md D44), and the program goes on;
//! while an import runs over several connections, each job's statement
//! (RULES.md D1148);
//! before, it ends the program, as it would have. Taken by tokio's signals,
//! on a thread of their own, so it is portable, SIGINT on Unix and the
//! console's Ctrl-C on Windows.

use std::future::Future;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::Poll;

use turborel_client::{Canceller, Failure};

/// What Ctrl-C cancels, once there is something.
#[derive(Debug, Clone, Default)]
pub struct Interrupts {
    cancellers: Arc<Mutex<Vec<Canceller>>>,
    /// How many times Ctrl-C was pressed once connected, so `\watch`
    /// knows to stop (RULES.md D215).
    presses: Arc<AtomicU64>,
}

/// What to do when Ctrl-C is pressed and no statement can be cancelled;
/// and with a failure to cancel one.
pub type Handlers = (fn(), fn(&Failure));

impl Interrupts {
    /// Takes Ctrl-C from now on, `handlers.0` called for it until there is
    /// a canceller, `handlers.1` with a failure to cancel.
    ///
    /// # Errors
    ///
    /// Why Ctrl-C cannot be taken.
    pub fn listen(handlers: Handlers) -> Result<Interrupts, String> {
        let interrupts = Interrupts::default();
        let (tell, told) = std::sync::mpsc::channel();
        let held = Arc::clone(&interrupts.cancellers);
        let presses = Arc::clone(&interrupts.presses);
        let spawned = std::thread::Builder::new().name("turborel-interrupts".to_owned()).spawn(move || {
            match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(runtime) => runtime.block_on(answer(tokio::signal::ctrl_c, (&held, &presses), handlers, tell)),
                Err(e) => drop(tell.send(Err(format!("cannot take Ctrl-C: {e}")))),
            }
        });
        spawned.map_err(|e| format!("cannot take Ctrl-C: {e}"))?;
        told.recv().map_err(|_| "cannot take Ctrl-C".to_owned())??;
        Ok(interrupts)
    }

    /// How many times Ctrl-C was pressed once there was a canceller.
    pub fn presses(&self) -> u64 {
        self.presses.load(Ordering::SeqCst)
    }

    /// Takes Ctrl-C as pressed once there is something to cancel.
    #[cfg(test)]
    pub(crate) fn press(&self) {
        self.presses.fetch_add(1, Ordering::SeqCst);
    }

    /// Cancels by `canceller` from now on, or, for none, by nothing. It is
    /// the current connection's, as SET CONNECTION and DISCONNECT change it.
    pub fn cancel_by(&self, canceller: Option<Canceller>) {
        self.cancel_each(canceller.into_iter().collect());
    }

    /// Cancels by each of `cancellers` from now on: the statements of an
    /// import's jobs, besides the current connection's (RULES.md D1148).
    pub fn cancel_each(&self, cancellers: Vec<Canceller>) {
        *self.cancellers.lock().unwrap_or_else(PoisonError::into_inner) = cancellers;
    }
}

/// Answers each signal `signal` gives until it fails, by each of
/// `cancellers` once there is one, else `handlers.0`. Tells `ready` once the first is awaited,
/// so a signal sent after that is answered; or why it cannot be.
pub(crate) async fn answer<F: Future<Output = io::Result<()>>>(
    mut signal: impl FnMut() -> F,
    (cancellers, presses): (&Mutex<Vec<Canceller>>, &AtomicU64),
    (early, failed): Handlers,
    ready: Sender<Result<(), String>>,
) {
    let mut ready = Some(ready);
    loop {
        let mut next = std::pin::pin!(signal());
        // Polled once, it is registered.
        let first = std::future::poll_fn(|cx| Poll::Ready(next.as_mut().poll(cx))).await;
        if let Some(tell) = ready.take() {
            let registered = match &first {
                Poll::Ready(Err(e)) => Err(format!("cannot take Ctrl-C: {e}")),
                Poll::Ready(Ok(())) | Poll::Pending => Ok(()),
            };
            // The program may have given up waiting.
            let _ = tell.send(registered);
        }
        let got = match first {
            Poll::Ready(got) => got,
            Poll::Pending => next.await,
        };
        if got.is_err() {
            return;
        }
        let by = cancellers.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if by.is_empty() {
            early();
            continue;
        }
        presses.fetch_add(1, Ordering::SeqCst);
        for e in by.iter().filter_map(|c| c.cancel().err()) {
            failed(&e);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use turborel::db::Database;
    use turborel_test_support::server::start;

    use super::*;

    static EARLY: AtomicUsize = AtomicUsize::new(0);
    static FAILED: AtomicUsize = AtomicUsize::new(0);

    fn early() {
        EARLY.fetch_add(1, Ordering::SeqCst);
    }

    fn failed(_: &Failure) {
        FAILED.fetch_add(1, Ordering::SeqCst);
    }

    /// Answers `signals` signals, then the signal fails; why it was told
    /// it cannot be ready, if it was.
    fn answered(signals: usize, cancellers: &Mutex<Vec<Canceller>>) -> Option<String> {
        let (tell, told) = std::sync::mpsc::channel();
        let mut left = signals;
        let signal = move || {
            let got = if left == 0 { Err(io::Error::other("gone")) } else { Ok(()) };
            left = left.saturating_sub(1);
            std::future::ready(got)
        };
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        runtime.block_on(answer(signal, (cancellers, &AtomicU64::new(0)), (early, failed), tell));
        told.recv().unwrap().err()
    }

    /// Verifies: [DESIGN] RULES.md D44, D46, D1148: each Ctrl-C cancels by
    /// the session's canceller once it has one, or by each job's of an
    /// import, a failure to cancel told of each; before, it is answered
    /// otherwise; Ctrl-C that cannot be taken is told.
    #[test]
    fn ctrl_c_cancels_once_connected() {
        let none = Mutex::new(Vec::new());
        assert_eq!(answered(2, &none), None);
        assert_eq!(EARLY.load(Ordering::SeqCst), 2, "before a connection");
        assert_eq!(answered(0, &none).as_deref(), Some("cannot take Ctrl-C: gone"));
        let server = start(&Database::new(), &[]);
        let connection = server.connect("X");
        let jobs = [server.connect("X"), server.connect("X")];
        let set = Mutex::new(connection.canceller().into_iter().collect());
        let each = Mutex::new(jobs.iter().filter_map(turborel_client::connection::Connection::canceller).collect());
        assert_eq!(answered(1, &set), None);
        assert_eq!(answered(1, &each), None);
        assert_eq!((EARLY.load(Ordering::SeqCst), FAILED.load(Ordering::SeqCst)), (2, 0), "cancelled");
        server.stop();
        assert_eq!(answered(1, &set), None);
        assert_eq!(FAILED.load(Ordering::SeqCst), 1, "the server is gone");
        assert_eq!(answered(1, &each), None);
        assert_eq!(FAILED.load(Ordering::SeqCst), 3, "each job's told");
        let interrupts = Interrupts::listen((early, failed)).unwrap();
        interrupts.cancel_by(connection.canceller());
        assert_eq!(interrupts.cancellers.lock().unwrap().len(), 1);
        interrupts.cancel_each(jobs.iter().filter_map(turborel_client::connection::Connection::canceller).collect());
        assert_eq!(interrupts.cancellers.lock().unwrap().len(), 2, "each job's");
        interrupts.cancel_by(None);
        assert!(interrupts.cancellers.lock().unwrap().is_empty(), "no connection current");
    }
}
