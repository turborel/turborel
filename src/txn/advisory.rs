//! \[EXTENSION\] Advisory locks (RULES.md D187): locks of keys an
//! application chooses, a number, a pair of numbers or a name, which mean
//! what it makes them mean. A session holds one to its end, or until it
//! unlocks it; a transaction holds one to its end. Each is exclusive or
//! shared, and taken again by its holder as many times as it is unlocked.
//! A wait for one goes through the graph of waits that table and row locks
//! use, so a cycle among them is a deadlock found at once, and ends at
//! LOCKS.TIMEOUT_MS or a cancel (`txn/waiting.rs`).

use std::collections::HashMap;
use std::fmt::{self, Display};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;

use super::waiting::{Waiter, Waits, wait_for};
use super::{Stop, TxId};
use crate::error::Result;
use crate::sync::{Signal, lock_builtin};

/// \[EXTENSION\] The key of an advisory lock.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum AdvisoryKey {
    /// A number.
    Int(i64),
    /// A pair of numbers, apart from every single number.
    Pair(i32, i32),
    /// A name.
    Name(String),
}

impl Display for AdvisoryKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AdvisoryKey::Int(n) => write!(f, "the advisory lock {n}"),
            AdvisoryKey::Pair(a, b) => write!(f, "the advisory lock ({a}, {b})"),
            AdvisoryKey::Name(name) => write!(f, "the advisory lock '{name}'"),
        }
    }
}

/// One session's hold of a key, at one scope and strength.
struct Holder {
    session: u64,
    /// Its session's flag, which says its transaction now.
    stop: Arc<Stop>,
    /// Of a transaction's hold, the transaction.
    xact: Option<TxId>,
    exclusive: bool,
    /// How many times it was taken and not unlocked.
    count: usize,
}

/// \[EXTENSION\] Who asks for an advisory lock: a transaction, its
/// session, and the hold it asks for.
pub(crate) struct Asker {
    pub(crate) waiter: Waiter,
    pub(crate) session: u64,
    /// For its transaction alone, else for its session.
    pub(crate) xact: bool,
    pub(crate) exclusive: bool,
}

/// \[EXTENSION\] The advisory locks sessions hold (RULES.md D187).
#[derive(Default)]
pub(crate) struct Advisory {
    held: Mutex<HashMap<AdvisoryKey, Vec<Holder>>>,
    released: Signal,
    /// How many holds are a transaction's, so that a transaction's end
    /// looks at none if none is.
    xacts: AtomicUsize,
}

impl Advisory {
    /// Takes `key` for `asker`, once no other session holds it against
    /// it, waiting as `waits` says: false if SKIP (a try) and it is held.
    ///
    /// # Errors
    ///
    /// As a lock's wait (`waiting::wait_for`).
    pub(crate) fn lock(&self, asker: &Asker, key: &AdvisoryKey, waits: Waits) -> Result<bool> {
        let against = |held: &HashMap<AdvisoryKey, Vec<Holder>>, _: ThreadId| -> Vec<(TxId, bool)> {
            let Some(holders) = held.get(key) else { return Vec::new() };
            holders
                .iter()
                .filter(|h| h.session != asker.session && (asker.exclusive || h.exclusive))
                .map(|h| (h.stop.tx.load(Ordering::SeqCst), false))
                .collect()
        };
        let (outcome, mut held) = wait_for((&asker.waiter, waits), key, (&self.held, &self.released), &against)?;
        if !outcome? {
            return Ok(false);
        }
        let xact = asker.xact.then_some(asker.waiter.id);
        let holders = held.entry(key.clone()).or_default();
        if let Some(mine) = holders.iter_mut().find(|h| h.session == asker.session && h.xact == xact && h.exclusive == asker.exclusive) {
            mine.count = mine.count.saturating_add(1);
            return Ok(true);
        }
        let stop = Arc::clone(&asker.waiter.stop);
        holders.push(Holder { session: asker.session, stop, xact, exclusive: asker.exclusive, count: 1 });
        if xact.is_some() {
            self.xacts.fetch_add(1, Ordering::SeqCst);
        }
        Ok(true)
    }

    /// Unlocks once session `session`'s own hold of `key`, exclusive or
    /// shared: false if it holds none.
    pub(crate) fn unlock(&self, session: u64, key: &AdvisoryKey, exclusive: bool) -> bool {
        let mut held = lock_builtin(&self.held);
        let Some(holders) = held.get_mut(key) else { return false };
        let Some(at) = holders.iter().position(|h| h.session == session && h.xact.is_none() && h.exclusive == exclusive) else {
            return false;
        };
        let left = holders.get_mut(at).map_or(0, |h| {
            h.count = h.count.saturating_sub(1);
            h.count
        });
        if left == 0 {
            holders.remove(at);
        }
        if holders.is_empty() {
            held.remove(key);
        }
        drop(held);
        self.released.notify_all();
        true
    }

    /// Unlocks every hold `keep` does not keep, waking those who wait.
    fn release(&self, keep: impl Fn(&Holder) -> bool) {
        let mut held = lock_builtin(&self.held);
        let mut xacts = 0_usize;
        held.retain(|_, holders| {
            holders.retain(|h| {
                let kept = keep(h);
                xacts = xacts.saturating_add(usize::from(!kept && h.xact.is_some()));
                kept
            });
            !holders.is_empty()
        });
        drop(held);
        self.xacts.fetch_sub(xacts, Ordering::SeqCst);
        self.released.notify_all();
    }

    /// Unlocks every hold of session `session`'s own: ADVISORY_UNLOCK_ALL.
    pub(crate) fn unlock_all(&self, session: u64) {
        self.release(|h| h.session != session || h.xact.is_some());
    }

    /// Unlocks every hold of transaction `tx`, as it ends.
    pub(crate) fn release_tx(&self, tx: TxId) {
        if self.xacts.load(Ordering::SeqCst) > 0 {
            self.release(|h| h.xact != Some(tx));
        }
    }

    /// Unlocks every hold of session `session`, as it ends.
    pub(crate) fn release_session(&self, session: u64) {
        self.release(|h| h.session != session);
    }
}
