//! \[DESIGN\] The server's time for a statement, which its completion
//! carries for a client granted `row-counts` (RULES.md D240). The clock
//! starts as a worker begins the statement's request and stops as its
//! completion is ready, the transaction it ends committed by then. A
//! cursor's time is summed over the Execute that opened it and each Fetch,
//! told by the Fetch that completes it; a copy's over each of its
//! requests, told by CopyEnd. Beside the whole, the time the request
//! waited, read, for a worker, and the time the statement waited for
//! locks, which a lock adds only once it was waited for. Not granted, no
//! clock is read.

use std::mem;
use std::time::{Duration, Instant};

use turborel_protocol::capability::ROW_COUNTS;
use turborel_protocol::{Completed, Reply, Timing};

use super::{Counted, Held};

/// A statement's clock: when a worker began it, and the microseconds its
/// request waited for a worker before.
pub(super) struct Clock {
    began: Instant,
    queued: u64,
}

/// `d` in microseconds.
fn micros(d: Duration) -> u64 {
    u64::try_from(d.as_micros()).unwrap_or(u64::MAX)
}

impl Held {
    /// Notes that a request was read now, if its time will be told.
    pub(in crate::native) fn arrived(&mut self) {
        if self.granted.has(ROW_COUNTS) {
            self.arrived = Some(Instant::now());
        }
    }

    /// Starts a statement's clock, if its time will be told: its request's
    /// wait for a worker its own if it is the request's first statement.
    pub(super) fn start(&mut self) {
        let arrived = self.arrived.take();
        self.clock = None;
        if !self.granted.has(ROW_COUNTS) {
            return;
        }
        let began = Instant::now();
        // A wait before it is not its own.
        self.session.canceller().served().take_lock_waits();
        let queued = arrived.map_or(0, |at| micros(began.saturating_duration_since(at)));
        self.clock = Some(Clock { began, queued });
    }

    /// The statement's time so far, the lock waits since taken; none if
    /// its time is not told.
    pub(super) fn so_far(&mut self) -> Option<Timing> {
        let clock = self.clock.take()?;
        let locks = self.session.canceller().served().take_lock_waits();
        Some(Timing { total: micros(clock.began.elapsed()), queued: clock.queued, locks })
    }

    /// Keeps a request's time that ends with no completion, of a copy under
    /// way, for its CopyEnd's.
    pub(super) fn kept(&mut self) {
        if let Some(t) = self.so_far().filter(|_| self.copy.is_some()) {
            self.carried = self.carried.plus(t);
        }
    }

    /// `last`, of work `counted`, carrying the statement's time if it is a
    /// completion of a statement, a cursor's fetch or a copy, and the time
    /// is told: with that of the cursor's earlier requests, or the copy's.
    /// A Close, a Deallocate or a request of a large object carries none.
    pub(super) fn timed(&mut self, last: Reply, counted: Counted) -> Reply {
        let earlier = mem::take(&mut self.carried);
        let Reply::Complete(c) = last else { return last };
        let carried = matches!(counted, Counted::Fetched | Counted::Copy);
        let counts = counted != Counted::Uncounted;
        let time = self.so_far().filter(|_| counts).map(|t| if carried { earlier.plus(t) } else { t });
        Reply::Complete(Completed { time, ..c })
    }
}
