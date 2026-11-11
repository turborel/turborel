//! \[DESIGN\] A condition variable that counts its waiters (RULES.md D27).
//! A notify of std's `Condvar` makes a system call whether or not a thread
//! waits, and the locks a writer takes and gives back on every change
//! (a tree's nodes, a file's turns, a frame's loading) mostly have none
//! waiting, so each notify cost a call that woke nobody.
//!
//! A waiter counts itself while it holds the mutex, before each wait, and
//! a notifier changes what is waited for under the same mutex before it
//! looks at the count. A waiter that counted itself then released the
//! mutex by waiting, so a notifier that took the mutex after it sees the
//! count; one that took the mutex before it changed the state first, and
//! the waiter sees the change and does not wait.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, LockResult, MutexGuard, PoisonError, WaitTimeoutResult};
use std::time::Duration;

/// \[DESIGN\] A `Condvar` whose notifies make no call while no thread waits.
/// Every change a waiter waits for must be made under the mutex it waits
/// with, as for any condition variable; a notify after a change made
/// without it may be skipped, so such a waiter must wait with a timeout.
#[derive(Default)]
pub(crate) struct Signal {
    condvar: Condvar,
    waiting: AtomicUsize,
}

/// A waiter counted until dropped, so a wait that returns, poisoned or
/// not, uncounts it.
struct Counted<'a>(&'a AtomicUsize);

impl<'a> Counted<'a> {
    fn new(waiting: &'a AtomicUsize) -> Counted<'a> {
        waiting.fetch_add(1, Ordering::SeqCst);
        Counted(waiting)
    }
}

impl Drop for Counted<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Signal {
    /// Waits for a notify, as `Condvar::wait`.
    ///
    /// # Errors
    ///
    /// As `Condvar::wait`, if the mutex is poisoned.
    pub(crate) fn wait<'a, T>(&self, guard: MutexGuard<'a, T>) -> LockResult<MutexGuard<'a, T>> {
        let _counted = Counted::new(&self.waiting);
        self.condvar.wait(guard)
    }

    /// Waits while `condition` holds, as `Condvar::wait_while`, counted only
    /// while it waits.
    ///
    /// # Errors
    ///
    /// As `Condvar::wait_while`, if the mutex is poisoned.
    pub(crate) fn wait_while<'a, T>(
        &self,
        mut guard: MutexGuard<'a, T>,
        mut condition: impl FnMut(&mut T) -> bool,
    ) -> LockResult<MutexGuard<'a, T>> {
        while condition(&mut *guard) {
            let _counted = Counted::new(&self.waiting);
            guard = self.condvar.wait(guard).map_err(|e| PoisonError::new(e.into_inner()))?;
        }
        Ok(guard)
    }

    /// Waits for a notify or until `timeout` passes, as
    /// `Condvar::wait_timeout`.
    ///
    /// # Errors
    ///
    /// As `Condvar::wait_timeout`, if the mutex is poisoned.
    pub(crate) fn wait_timeout<'a, T>(
        &self,
        guard: MutexGuard<'a, T>,
        timeout: Duration,
    ) -> LockResult<(MutexGuard<'a, T>, WaitTimeoutResult)> {
        let _counted = Counted::new(&self.waiting);
        self.condvar.wait_timeout(guard, timeout)
    }

    /// Wakes every waiter, if one waits.
    pub(crate) fn notify_all(&self) {
        if self.waiting.load(Ordering::SeqCst) > 0 {
            self.condvar.notify_all();
        }
    }

    /// Wakes one waiter, if one waits.
    pub(crate) fn notify_one(&self) {
        if self.waiting.load(Ordering::SeqCst) > 0 {
            self.condvar.notify_one();
        }
    }

    /// The threads waiting now.
    #[cfg(test)]
    fn waiting(&self) -> usize {
        self.waiting.load(Ordering::SeqCst)
    }
}

// Unit tests: waiters are counted while they wait, and a notify reaches
// each (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
