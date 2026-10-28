//! \[DESIGN\] What Ctrl-C does (RULES.md D46): once connected, it cancels
//! the statement the session runs (RULES.md D44), and the program goes on;
//! before, it ends the program, as it would have. Taken by tokio's signals,
//! on a thread of their own, so it is portable: SIGINT on Unix, the
//! console's Ctrl-C on Windows.

use std::future::Future;
use std::io;
use std::sync::mpsc::Sender;
use std::sync::{Arc, OnceLock};
use std::task::Poll;

use turborel_client::{Canceller, Failure};

/// What Ctrl-C cancels, once there is something.
#[derive(Debug, Clone, Default)]
pub struct Interrupts {
    canceller: Arc<OnceLock<Canceller>>,
}

/// What to do when Ctrl-C is pressed and no statement can be cancelled;
/// and with a failure to cancel one.
pub type Handlers = (fn(), fn(&Failure));

impl Interrupts {
    /// Takes Ctrl-C from now on: `handlers.0` called for it until there is
    /// a canceller, `handlers.1` with a failure to cancel.
    ///
    /// # Errors
    ///
    /// Why Ctrl-C cannot be taken.
    pub fn listen(handlers: Handlers) -> Result<Interrupts, String> {
        let interrupts = Interrupts::default();
        let (tell, told) = std::sync::mpsc::channel();
        let held = Arc::clone(&interrupts.canceller);
        let spawned = std::thread::Builder::new().name("interrupts".to_owned()).spawn(move || {
            match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(runtime) => runtime.block_on(answer(tokio::signal::ctrl_c, &held, handlers, tell)),
                Err(e) => drop(tell.send(Err(format!("cannot take Ctrl-C: {e}")))),
            }
        });
        spawned.map_err(|e| format!("cannot take Ctrl-C: {e}"))?;
        told.recv().map_err(|_| "cannot take Ctrl-C".to_owned())??;
        Ok(interrupts)
    }

    /// Cancels by `canceller` from now on.
    pub fn cancel_by(&self, canceller: Canceller) {
        // Set once: a session has one canceller.
        let _ = self.canceller.set(canceller);
    }
}

/// Answers each signal `signal` gives until it fails: by `canceller`, once
/// it is set, else `handlers.0`. Tells `ready` once the first is awaited,
/// so a signal sent after that is answered; or why it cannot be.
pub(crate) async fn answer<F: Future<Output = io::Result<()>>>(
    mut signal: impl FnMut() -> F,
    canceller: &OnceLock<Canceller>,
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
        match canceller.get() {
            Some(by) => {
                if let Err(e) = by.cancel() {
                    failed(&e);
                }
            }
            None => early(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

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
    fn answered(signals: usize, canceller: &OnceLock<Canceller>) -> Option<String> {
        let (tell, told) = std::sync::mpsc::channel();
        let mut left = signals;
        let signal = move || {
            let got = if left == 0 { Err(io::Error::other("gone")) } else { Ok(()) };
            left = left.saturating_sub(1);
            std::future::ready(got)
        };
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        runtime.block_on(answer(signal, canceller, (early, failed), tell));
        told.recv().unwrap().err()
    }

    /// Verifies: [DESIGN] RULES.md D44, D46: each Ctrl-C cancels by the
    /// session's canceller once it has one, a failure to cancel told; before,
    /// it is answered otherwise; Ctrl-C that cannot be taken is told.
    #[test]
    fn ctrl_c_cancels_once_connected() {
        let none = OnceLock::new();
        assert_eq!(answered(2, &none), None);
        assert_eq!(EARLY.load(Ordering::SeqCst), 2, "before a connection");
        assert_eq!(answered(0, &none).as_deref(), Some("cannot take Ctrl-C: gone"));
        let server = start(&Database::new(), &[]);
        let connection = server.connect("X");
        let set = OnceLock::new();
        let _ = set.set(connection.canceller().unwrap());
        assert_eq!(answered(1, &set), None);
        assert_eq!((EARLY.load(Ordering::SeqCst), FAILED.load(Ordering::SeqCst)), (2, 0), "cancelled");
        server.stop();
        assert_eq!(answered(1, &set), None);
        assert_eq!(FAILED.load(Ordering::SeqCst), 1, "the server is gone");
        let interrupts = Interrupts::listen((early, failed)).unwrap();
        interrupts.cancel_by(connection.canceller().unwrap());
        assert!(interrupts.canceller.get().is_some());
    }
}
