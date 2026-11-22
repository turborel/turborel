//! \[DESIGN\] A connection's home (RULES.md D767, D768): one thread of the
//! server's blocking pool that runs every request of the connection for as
//! long as the connection keeps it busy. The
//! memory a statement takes is then given back on the thread that took it,
//! from that thread's own cache in the allocator, and what the session
//! keeps between statements stays in that thread's processor caches. A
//! request handed to any free thread of the pool would move the session
//! between threads at every request, each freeing what another allocated.
//!
//! The connection's task hands each job over with its permit already
//! taken, so SERVER.WORKERS bounds statements as before, and waits for the
//! loan back as before (`native/lent.rs`). Between jobs the thread parks,
//! holding no permit. It retires once no job has come for `KEPT`, its
//! retirement and a hand-over decided under one lock, so a job is never
//! handed to a thread that has gone; a connection whose home retired opens
//! another at its next request. A home holds a place among the threads the
//! connections keep (`Services::hot`) while it lives, so the pool always
//! has a thread for each statement allowed beside them; a connection that
//! finds no place has its requests run on any free thread of the pool.

use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crate::limit::Held;

/// \[DESIGN\] How long a home waits for its connection's next job before
/// it retires: as long as the runtime keeps an idle thread of its pool,
/// so a connection idle that long gives its thread back as the pool would
/// have. Fixed, not a setting (RULES.md D36): the shape of a hand-over
/// between threads, which no work of the server's tunes.
pub(super) const KEPT: Duration = Duration::from_secs(10);

/// \[DESIGN\] What a home runs.
pub(super) trait Job: Send + 'static {
    /// Runs it, on the home's thread.
    fn run(self);
}

/// What the connection and its home share, under the home's lock.
struct State<J> {
    /// The job handed over and not yet taken.
    job: Option<J>,
    /// The home's thread has gone, or is going: it takes no more jobs.
    retired: bool,
    /// The connection has gone: the thread retires once it has nothing to do.
    closed: bool,
}

/// The home's lock, and what tells its thread of a job or the end.
struct Shared<J> {
    state: Mutex<State<J>>,
    told: Condvar,
}

impl<J> Shared<J> {
    fn locked(&self) -> MutexGuard<'_, State<J>> {
        // A job runs with the lock not held, and the state is changed in
        // single assignments, so a poisoned lock is whole.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A connection's home, as its task holds it: closed when dropped.
pub(super) struct Home<J: Job> {
    shared: Arc<Shared<J>>,
}

impl<J: Job> Home<J> {
    /// A home on a thread of the runtime's blocking pool, holding `place`
    /// while it lives, retiring once no job has come for `kept`.
    pub(super) fn open(place: Held, kept: Duration) -> Home<J> {
        let shared = Arc::new(Shared { state: Mutex::new(State { job: None, retired: false, closed: false }), told: Condvar::new() });
        let thread = Thread { shared: Arc::clone(&shared), kept, _place: place };
        // Not waited for: the thread ends as the home retires or closes.
        // A runtime stopping drops it unrun, which retires the home.
        drop(tokio::task::spawn_blocking(move || thread.serve()));
        Home { shared }
    }

    /// Hands `job` to the home's thread; given back if it has retired.
    ///
    /// # Errors
    ///
    /// `job` itself, the home having retired.
    pub(super) fn send(&self, job: J) -> Result<(), J> {
        let mut state = self.shared.locked();
        if state.retired {
            return Err(job);
        }
        state.job = Some(job);
        drop(state);
        self.shared.told.notify_one();
        Ok(())
    }
}

impl<J: Job> Drop for Home<J> {
    fn drop(&mut self) {
        self.shared.locked().closed = true;
        self.shared.told.notify_one();
    }
}

/// The home, as its thread holds it: retired as this is dropped, whether
/// it served to its end, panicked, or never ran.
struct Thread<J> {
    shared: Arc<Shared<J>>,
    kept: Duration,
    /// Its place among the threads connections keep, given back with it.
    _place: Held,
}

impl<J: Job> Thread<J> {
    /// Runs each job handed over, until the home retires.
    fn serve(self) {
        while let Some(job) = self.next() {
            job.run();
        }
    }

    /// The next job, once handed over; none once the connection has gone,
    /// or none came within `kept`, the home then retired under the lock
    /// that hands one over.
    fn next(&self) -> Option<J> {
        let mut state = self.shared.locked();
        loop {
            if let Some(job) = state.job.take() {
                return Some(job);
            }
            if state.closed {
                state.retired = true;
                return None;
            }
            let (back, waited) = self.shared.told.wait_timeout(state, self.kept).unwrap_or_else(PoisonError::into_inner);
            state = back;
            if waited.timed_out() && state.job.is_none() {
                state.retired = true;
                return None;
            }
        }
    }
}

impl<J> Drop for Thread<J> {
    fn drop(&mut self) {
        let mut state = self.shared.locked();
        state.retired = true;
        // A job handed over and never run, as the runtime stops: dropped
        // with the lock not held, so what it gives back on its drop (a
        // loan, failed) takes no lock of the home's.
        let unrun = state.job.take();
        drop(state);
        drop(unrun);
    }
}

#[cfg(test)]
mod tests;
