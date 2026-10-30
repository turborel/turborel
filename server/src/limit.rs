//! \[DESIGN\] A limit on how many of something are held at once, read anew
//! each time one is taken, so a setting that changes moves it: raised, it
//! lets more in as soon as one is taken or given back; lowered, it lets
//! none in until enough are given back. A semaphore's permits are fixed.

use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::Notify;

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
        self.0.freed.notify_waiters();
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
}
