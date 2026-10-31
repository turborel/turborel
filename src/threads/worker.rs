//! \[DESIGN\] A job's thread (RULES.md D55): it waits until its job is
//! asked for, at once or at a time asked, or until the time the job's
//! work last gave passes; does it;
//! and ends when told, or when the database is gone. It holds the database
//! weakly, so never keeps it open between rounds. Split from threads.rs
//! (RULES.md R-SIZE-2).

use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::Job;
use crate::db::WeakDatabase;
use crate::error::{Code, Result, SqlError};
use crate::sync::lock_builtin;

/// A job's thread, if it has one, and what wakes it.
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

/// Why the thread wakes: its work asked for, now or at a time, or to end.
#[derive(Default)]
struct Wake {
    due: bool,
    at: Option<Instant>,
    stop: bool,
}

impl Signal {
    /// Sets what `change` sets, and wakes the thread.
    fn set(&self, change: impl FnOnce(&mut Wake)) {
        change(&mut self.state.lock().unwrap_or_else(PoisonError::into_inner));
        self.wake.notify_one();
    }

    /// Waits until the work is asked for, the time it was asked for at
    /// comes, or `every` passes if given: whether to do it, not to end.
    fn wait(&self, every: Option<Duration>) -> bool {
        let until = every.and_then(|e| Instant::now().checked_add(e));
        let mut wake = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        while !wake.due && !wake.stop {
            let Some(next) = until.into_iter().chain(wake.at).min() else {
                wake = self.wake.wait(wake).unwrap_or_else(PoisonError::into_inner);
                continue;
            };
            let Some(left) = next.checked_duration_since(Instant::now()).filter(|l| !l.is_zero()) else { break };
            wake = self.wake.wait_timeout(wake, left).unwrap_or_else(PoisonError::into_inner).0;
        }
        wake.due = false;
        wake.at = None;
        !wake.stop
    }
}

impl Worker {
    /// Starts the thread, labelled `label`, doing `job` for `db`, unless it
    /// is running: whether it started one.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if the thread cannot be started.
    pub(super) fn start(&self, label: &str, db: WeakDatabase, job: Arc<Job>) -> Result<bool> {
        let mut slot = lock_builtin(&self.thread);
        if slot.as_ref().is_some_and(|t| !t.is_finished()) {
            return Ok(false);
        }
        self.signal.set(|w| *w = Wake::default());
        let signal = Arc::clone(&self.signal);
        let thread = thread::Builder::new()
            .name(label.into())
            .spawn(move || run(&db, &signal, &job))
            .map_err(|e| SqlError::new(Code::Io, format!("[DESIGN] the thread {label} could not be started: {e}")))?;
        *slot = Some(thread);
        Ok(true)
    }

    /// Asks the thread for its work, `after` from now: at once if zero; a
    /// time asked before kept if sooner. A time past what an instant holds
    /// asks for none.
    pub(super) fn request(&self, after: Duration) {
        if after.is_zero() {
            self.signal.set(|w| w.due = true);
            return;
        }
        let Some(at) = Instant::now().checked_add(after) else { return };
        self.signal.set(|w| w.at = Some(w.at.map_or(at, |was| was.min(at))));
    }

    /// Whether its thread runs.
    pub(super) fn running(&self) -> bool {
        lock_builtin(&self.thread).as_ref().is_some_and(|t| !t.is_finished())
    }

    /// Ends the thread, after the work it is doing, if any, and joins it:
    /// whether there was one. Started again, it runs as before.
    pub(super) fn stop(&self) -> bool {
        self.signal.set(|w| w.stop = true);
        let Some(thread) = lock_builtin(&self.thread).take() else { return false };
        // The thread itself, ending as it is told: it cannot join itself.
        // Its database never goes on it (`db/database/handles.rs`), but
        // its work may stop its own job, as a policy taken there would.
        if thread.thread().id() != thread::current().id() {
            let _ = thread.join();
        }
        true
    }
}

/// Dropped with the database: the thread is told to end, and waited for.
/// The program's last handle, dropped, joined it already
/// (`db/database/handles.rs`): this keeps a thread from outliving its job
/// whatever drops it.
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A job's thread: does its work each time it wakes, until told to end, or
/// the database is gone.
fn run(db: &WeakDatabase, signal: &Signal, job: &Job) {
    let mut every = None;
    while signal.wait(every) {
        let Some(db) = db.upgrade() else { return };
        let done = job.run(&db);
        every = job.next(&done);
    }
}
