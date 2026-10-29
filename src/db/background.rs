//! \[DESIGN\] The database's own threads: its checkpoint thread
//! (`db/checkpoint.rs`) and its maintenance thread (`db/maintenance.rs`).
//! Each waits until it is asked for its work, or until the time its work
//! last gave passes; does it; and ends when told, or when the database is
//! gone: it holds the database weakly, so it never keeps it open.

use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::Database;
use super::database::WeakDatabase;
use crate::error::{Code, Result, SqlError};
use crate::sync::lock_builtin;

/// The work of a thread, done on each waking: how long it may wait before
/// it is done again unasked, if at all.
pub(super) type Work = fn(&Database) -> Option<Duration>;

/// \[DESIGN\] One of the database's threads.
#[derive(Default)]
pub(super) struct Worker {
    signal: Arc<Signal>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

/// What wakes the thread.
#[derive(Default)]
struct Signal {
    state: Mutex<Wake>,
    wake: Condvar,
}

/// Why the thread wakes: its work asked for, or to end.
#[derive(Default)]
struct Wake {
    due: bool,
    stop: bool,
}

impl Signal {
    /// Sets what `change` sets, and wakes the thread.
    fn set(&self, change: impl FnOnce(&mut Wake)) {
        change(&mut self.state.lock().unwrap_or_else(PoisonError::into_inner));
        self.wake.notify_one();
    }

    /// Waits until the work is asked for, or `every` passes if given:
    /// whether to do it, not to end.
    fn wait(&self, every: Option<Duration>) -> bool {
        let mut wake = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        while !wake.due && !wake.stop {
            let Some(every) = every else {
                wake = self.wake.wait(wake).unwrap_or_else(PoisonError::into_inner);
                continue;
            };
            let (woken, waited) = self.wake.wait_timeout(wake, every).unwrap_or_else(PoisonError::into_inner);
            wake = woken;
            if waited.timed_out() {
                break;
            }
        }
        wake.due = false;
        !wake.stop
    }
}

impl Worker {
    /// Starts the thread, named `name`, doing `work` for `db`, unless it
    /// is running.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if the thread cannot be started.
    pub(super) fn start(&self, name: &str, db: WeakDatabase, work: Work) -> Result<()> {
        let mut slot = lock_builtin(&self.thread);
        if slot.as_ref().is_some_and(|t| !t.is_finished()) {
            return Ok(());
        }
        let signal = Arc::clone(&self.signal);
        let thread = thread::Builder::new()
            .name(name.into())
            .spawn(move || run(&db, &signal, work))
            .map_err(|e| SqlError::new(Code::Io, format!("[DESIGN] the thread {name} could not be started: {e}")))?;
        *slot = Some(thread);
        Ok(())
    }

    /// Asks the thread for its work.
    pub(super) fn request(&self) {
        self.signal.set(|w| w.due = true);
    }

    /// Ends the thread, after the work it is doing, if any; started again,
    /// it runs as before.
    pub(super) fn stop(&self) {
        self.signal.set(|w| w.stop = true);
        let thread = lock_builtin(&self.thread).take();
        // The thread itself, ending as it is told.
        if thread.as_ref().is_some_and(|t| t.thread().id() == thread::current().id()) {
            return;
        }
        if let Some(thread) = thread {
            let _ = thread.join();
        }
        self.signal.set(|w| *w = Wake::default());
    }
}

/// Dropped with the database: the thread is told to end, and waited for,
/// unless the thread itself dropped the database's last handle.
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A thread of the database: does `work` each time it wakes, until told to
/// end, or the database is gone.
fn run(db: &WeakDatabase, signal: &Signal, work: Work) {
    let mut every = None;
    while signal.wait(every) {
        let Some(db) = db.upgrade() else { return };
        every = work(&db);
    }
}
