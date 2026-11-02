//! \[EXTENSION\] The driver's seams (RULES.md D57, R-SAFE-3), each a trait
//! or a part a transport and a host fill: where frames arrive (`Inbox`),
//! how time passes (`Ticker`), how a sync waits for its commit (`Wait`),
//! and what is told of what the driver carries out (`Watch`). Built in:
//! ticks of a real interval (`Ticks`), and a wait that blocks (`Blocking`);
//! the simulator of `turborel-test-support` gives its own.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use turborel::error::Result;

use super::Driver;
use crate::{Lsn, ReplicaEvent, ReplicaId};

/// \[EXTENSION\] Frames arrived for a replica, and not yet taken by its
/// driver: a transport puts them here, in the order they arrived.
#[derive(Default)]
pub struct Inbox {
    frames: Mutex<VecDeque<(ReplicaId, Vec<u8>)>>,
    arrived: Condvar,
}

impl Inbox {
    /// `frame` arrived from `from`.
    pub fn push(&self, from: ReplicaId, frame: Vec<u8>) {
        self.locked().push_back((from, frame));
        self.arrived.notify_all();
    }

    /// The frames arrived, oldest first, taken.
    pub fn take(&self) -> Vec<(ReplicaId, Vec<u8>)> {
        self.locked().drain(..).collect()
    }

    /// Lets go of every frame waiting: its replica stopped.
    pub fn clear(&self) {
        self.locked().clear();
    }

    /// Waits until a frame waits, or `most` passes.
    pub fn wait(&self, most: Duration) {
        let frames = self.locked();
        if frames.is_empty() {
            drop(self.arrived.wait_timeout(frames, most).unwrap_or_else(PoisonError::into_inner));
        }
    }

    /// The frames, locked; each change leaves them whole.
    fn locked(&self) -> std::sync::MutexGuard<'_, VecDeque<(ReplicaId, Vec<u8>)>> {
        self.frames.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// \[EXTENSION\] How time passes for a driver: the ticks of its consensus
/// due since it last asked.
pub trait Ticker: Send + Sync {
    /// The ticks due since the last call, taken.
    fn take(&self) -> u64;
}

/// \[EXTENSION\] Ticks of a real interval, from when they are made.
pub struct Ticks {
    interval: Duration,
    began: Instant,
    taken: Mutex<u64>,
}

impl Ticks {
    /// A tick every `interval`, from now.
    pub fn every(interval: Duration) -> Ticks {
        Ticks { interval, began: Instant::now(), taken: Mutex::new(0) }
    }

    /// The interval between ticks.
    pub fn interval(&self) -> Duration {
        self.interval
    }
}

impl Ticker for Ticks {
    fn take(&self) -> u64 {
        let nanos = self.interval.as_nanos().max(1);
        let passed = u64::try_from(self.began.elapsed().as_nanos().checked_div(nanos).unwrap_or(0)).unwrap_or(u64::MAX);
        let mut taken = self.taken.lock().unwrap_or_else(PoisonError::into_inner);
        let due = passed.saturating_sub(*taken);
        *taken = passed.max(*taken);
        due
    }
}

/// \[EXTENSION\] How a sync of the driver's log waits for its commit:
/// each call lets the driver move on a step, or waits for another thread
/// to move it; the sync then looks again.
pub trait Wait: Send + Sync {
    /// Waits a step for `driver`.
    ///
    /// # Errors
    ///
    /// The driver's, moving it on.
    fn wait(&self, driver: &Arc<Driver>) -> Result<()>;
}

/// \[EXTENSION\] The built-in wait: on the thread carrying out the
/// driver's effects (a promotion syncing its abort records, on the
/// driver's job), it moves the driver's inputs on itself, waiting up to
/// `most` for a frame; on any other, it waits up to `most` for the driver
/// to move on, which its job does.
pub struct Blocking {
    /// The longest it waits a step.
    pub most: Duration,
}

impl Wait for Blocking {
    fn wait(&self, driver: &Arc<Driver>) -> Result<()> {
        if driver.applying_here() {
            driver.step_inputs()?;
            driver.inbox().wait(self.most);
            return driver.step_inputs();
        }
        driver.wait_changed(self.most);
        Ok(())
    }
}

/// \[EXTENSION\] What a driver tells of what it carries out, for the
/// operator, or a simulator's checks. Called with the driver's lock held:
/// it never calls back into the driver (RULES.md R-ERR-4).
pub trait Watch: Send + Sync {
    /// The log through `through` is known committed here, as `log` holds
    /// it after `first`, the records through `first` let go: told to a
    /// serving primary, or delivered to the database.
    fn committed(&self, window: (Lsn, Lsn), log: &[Vec<u8>]);
    /// The database serves as primary of `view`.
    fn serving(&self, view: u64);
    /// The database stopped serving, deposed and opened again.
    fn stopped(&self);
    /// Whether the log kept `record`, appended at `at` in a service ended;
    /// unknown if a state transfer installed let the position go first.
    fn outcome(&self, at: Lsn, record: &[u8], kept: Option<bool>);
    /// The consensus reported `event`.
    fn event(&self, event: &ReplicaEvent);
    /// \[EXTENSION\] The database installed a state transfer, holding the
    /// log through `at` (RULES.md D57). Default: nothing.
    fn installed(&self, _at: Lsn) {}
}

/// \[EXTENSION\] A watch that tells nothing.
pub struct Unwatched;

impl Watch for Unwatched {
    fn committed(&self, _window: (Lsn, Lsn), _log: &[Vec<u8>]) {}
    fn serving(&self, _view: u64) {}
    fn stopped(&self) {}
    fn outcome(&self, _at: Lsn, _record: &[u8], _kept: Option<bool>) {}
    fn event(&self, _event: &ReplicaEvent) {}
}
