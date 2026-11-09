//! \[DESIGN\] A limit on how many of something are held at once, read anew
//! each time one is taken, so a setting that changes moves it: raised, it
//! lets more in as soon as one is taken or given back; lowered, it lets
//! none in until enough are given back. A semaphore's permits are fixed.
//! A worker's permit (`Permit`) is one of the limit of SERVER.WORKERS,
//! given up while the worker waits on its client, and taken again.
//!
//! One given back wakes one waiter, not every one, as only one can take
//! it: a waiter woken that finds none, another having taken it first,
//! waits again, and one dropped as it is woken passes the wake on
//! (`Notify::notify_one`). One taken that leaves room wakes the next, so
//! a limit raised lets in as many as it allows.

use std::sync::{Arc, Mutex, PoisonError};

use tokio::runtime::Handle;
use tokio::sync::Notify;
use turborel::db::Database;

use crate::settings::{WORKERS, count};
use crate::shown::{Counted, Shown};

/// What is held, and who waits.
#[derive(Default)]
pub(crate) struct Limit {
    held: Mutex<usize>,
    freed: Notify,
}

/// One held, given back when dropped.
pub(crate) struct Held(Arc<Limit>);

impl Limit {
    /// One, if fewer than `limit` are held.
    pub(crate) fn try_take(self: &Arc<Self>, limit: usize) -> Option<Held> {
        // Nothing panics while it is locked, so a poisoned lock is whole.
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        if *held >= limit {
            return None;
        }
        *held = held.saturating_add(1);
        let room = *held < limit;
        drop(held);
        if room {
            self.freed.notify_one();
        }
        Some(Held(Arc::clone(self)))
    }

    /// One, once fewer than `limit()` are held: read each time one is
    /// given back.
    pub(crate) async fn take(self: &Arc<Self>, limit: impl Fn() -> usize) -> Held {
        loop {
            // Waiting begins before the count is read, so one given back
            // between the two is not missed.
            let freed = self.freed.notified();
            tokio::pin!(freed);
            freed.as_mut().enable();
            if let Some(held) = self.try_take(limit()) {
                return held;
            }
            freed.await;
        }
    }

    /// How many are held.
    pub(crate) fn held(&self) -> usize {
        *self.held.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        let mut held = self.0.held.lock().unwrap_or_else(PoisonError::into_inner);
        *held = held.saturating_sub(1);
        drop(held);
        self.0.freed.notify_one();
    }
}

/// \[DESIGN\] A worker's permit to run a statement, one of SERVER.WORKERS,
/// counted busy while it holds it: given up while the worker waits on its
/// client for the next request, and taken again, counted waiting until it
/// is, for the request that came (`native/lent.rs`).
pub struct Permit {
    workers: Arc<Limit>,
    shown: Arc<Shown>,
    db: Database,
    held: Option<(Held, Counted)>,
}

impl Permit {
    /// `held`, one of `workers`, as many as SERVER.WORKERS of `db` says,
    /// counted busy in `shown` while it is held.
    pub(crate) fn new(workers: &Arc<Limit>, shown: &Arc<Shown>, db: &Database, held: Held) -> Permit {
        let busy = Counted::of(shown, |s| &s.busy);
        Permit { workers: Arc::clone(workers), shown: Arc::clone(shown), db: db.clone(), held: Some((held, busy)) }
    }

    /// Gives the permit up, for another statement.
    pub fn release(&mut self) {
        self.held = None;
    }

    /// Takes a permit again, once fewer statements run than SERVER.WORKERS
    /// says; waiting on `handle`, the runtime's, from a thread apart from
    /// it.
    pub fn retake(&mut self, handle: &Handle) {
        if self.held.is_some() {
            return;
        }
        let most = || count(&self.db, WORKERS);
        let held = self.workers.try_take(most()).unwrap_or_else(|| {
            let _waiting = Counted::of(&self.shown, |s| &s.waiting);
            handle.block_on(self.workers.take(most))
        });
        self.held = Some((held, Counted::of(&self.shown, |s| &s.busy)));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// Verifies: [DESIGN] RULES.md D42: no more are held than the limit;
    /// one given back lets a waiter in; a limit raised lets more in, read
    /// when one is taken; a limit lowered lets none in until enough are
    /// given back.
    #[test]
    fn a_limit_is_read_each_time_one_is_taken() {
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let limit = Arc::new(Limit::default());
        let first = limit.try_take(1).unwrap();
        assert!(limit.try_take(1).is_none());
        let allowed = Arc::new(AtomicUsize::new(1));
        let reader = Arc::clone(&allowed);
        let second = runtime.block_on(async {
            let waiting = tokio::spawn({
                let limit = Arc::clone(&limit);
                async move { limit.take(move || reader.load(Ordering::SeqCst)).await }
            });
            // The waiter runs, and waits, before the first is given back.
            tokio::task::yield_now().await;
            assert!(!waiting.is_finished());
            drop(first);
            waiting.await.unwrap()
        });
        assert_eq!(limit.held(), 1);
        allowed.store(3, Ordering::SeqCst);
        let third = runtime.block_on(limit.take(|| allowed.load(Ordering::SeqCst)));
        assert_eq!(limit.held(), 2);
        assert!(limit.try_take(2).is_none(), "lowered");
        drop((second, third));
        assert_eq!(limit.held(), 0);
        assert!(limit.try_take(2).is_some());
    }

    /// How many of two waiters on a limit of one, held, are let in as the
    /// one held is given back, the limit `raised` meanwhile.
    fn let_in(runtime: &tokio::runtime::Runtime, raised: usize) -> usize {
        let limit = Arc::new(Limit::default());
        let allowed = Arc::new(AtomicUsize::new(1));
        let first = limit.try_take(1).unwrap();
        let wait = || {
            let (limit, allowed) = (Arc::clone(&limit), Arc::clone(&allowed));
            async move { limit.take(move || allowed.load(Ordering::SeqCst)).await }
        };
        runtime.block_on(async {
            let waiters = [tokio::spawn(wait()), tokio::spawn(wait())];
            tokio::task::yield_now().await;
            assert!(waiters.iter().all(|w| !w.is_finished()), "both wait");
            allowed.store(raised, Ordering::SeqCst);
            drop(first);
            for _ in 0_u8..4 {
                tokio::task::yield_now().await;
            }
            let finished = waiters.iter().filter(|w| w.is_finished()).count();
            waiters.iter().for_each(tokio::task::JoinHandle::abort);
            finished
        })
    }

    /// Verifies: [DESIGN] RULES.md D42: one given back lets in one of two
    /// waiters, the other waiting still; with the limit raised meanwhile,
    /// the one let in, leaving room, lets the other in too. MC/DC of the
    /// wake on a take: room left (the second let in) against none left (it
    /// waits).
    #[test]
    fn one_given_back_lets_in_one_waiter_and_room_left_the_next() {
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        assert_eq!(let_in(&runtime, 1), 1_usize, "no room left");
        assert_eq!(let_in(&runtime, 2), 2_usize, "room left");
    }
}
