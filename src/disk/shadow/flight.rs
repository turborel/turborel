//! \[DESIGN\] Runs written over in place outside a shadow file's lock
//! (RULES.md D30, D472): the granules being written, which a reader of
//! one of them waits for, and every change but such a write waits for
//! all of. A page written back again writes its block with no lock held,
//! so reads of the file's other pages go on meanwhile.

use std::collections::HashSet;
use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, PoisonError};

use crate::sync::lock_builtin;

/// The granules of a shadow file being written outside its lock.
pub(super) struct Flight {
    granules: Mutex<HashSet<u64>>,
    /// How many, read with no lock: none, the common case, waits for none.
    flying: AtomicUsize,
    landed: Condvar,
}

impl Flight {
    /// No granule in flight.
    pub(super) fn new() -> Flight {
        Flight { granules: Mutex::new(HashSet::new()), flying: AtomicUsize::new(0), landed: Condvar::new() }
    }

    /// Marks `numbers` in flight, under the file's lock written; false,
    /// marking none, if one already is.
    pub(super) fn take_off(&self, numbers: Range<u64>) -> bool {
        let mut granules = lock_builtin(&self.granules);
        if numbers.clone().any(|n| granules.contains(&n)) {
            return false;
        }
        granules.extend(numbers);
        self.flying.store(granules.len(), Ordering::Release);
        true
    }

    /// Marks `numbers` written, waking those that wait for them.
    pub(super) fn land(&self, numbers: Range<u64>) {
        let mut granules = lock_builtin(&self.granules);
        for n in numbers {
            granules.remove(&n);
        }
        self.flying.store(granules.len(), Ordering::Release);
        drop(granules);
        self.landed.notify_all();
    }

    /// Waits until granule `number` is not in flight. A writer in flight
    /// takes no lock of the file to land, so a reader may wait holding it.
    pub(super) fn wait_for(&self, number: u64) {
        self.wait_until(|granules| !granules.contains(&number));
    }

    /// Waits until no granule is in flight: for a change of the file, its
    /// lock written, so none takes off meanwhile.
    pub(super) fn wait_all(&self) {
        self.wait_until(HashSet::is_empty);
    }

    /// Waits until `done` holds of the granules in flight.
    fn wait_until(&self, done: impl Fn(&HashSet<u64>) -> bool) {
        if self.flying.load(Ordering::Acquire) == 0 {
            return;
        }
        let mut granules = lock_builtin(&self.granules);
        while !done(&granules) {
            granules = self.landed.wait(granules).unwrap_or_else(PoisonError::into_inner);
        }
    }
}
