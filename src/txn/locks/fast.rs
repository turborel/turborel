//! \[DESIGN\] Shared holds of a table's lock, in stripes (RULES.md D442).
//! Every writer of a table whose engine keeps versions holds its lock
//! shared to its end, so on a hot table each one took the same holders'
//! lock twice. A shared hold is taken instead in one of `STRIPES` lists,
//! chosen by the transaction's id, while no transaction holds the lock
//! exclusively or waits to. An exclusive request first marks the lock
//! blocked, under the holders' lock, and then waits for the stripes to
//! hold no one but itself, naming each holder for the graph of who waits
//! for whom. A shared request reads the mark under its stripe's lock, so
//! either the request sees the mark and takes the slow path, or the
//! exclusive request sees it in its stripe.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::Holder;
use crate::sync::lock_builtin;
use crate::txn::{LockMode, TxId, Txn};

/// Stripes of a lock's shared holds.
const STRIPES: u64 = 16;

/// The shared holds of one table's lock taken in stripes.
pub(super) struct Stripes {
    stripes: Box<[Mutex<Vec<Holder>>]>,
    /// Exclusive holders and requests: while any, no hold is taken here.
    blocked: AtomicUsize,
}

impl Default for Stripes {
    fn default() -> Stripes {
        Stripes { stripes: (0..STRIPES).map(|_| Mutex::new(Vec::new())).collect(), blocked: AtomicUsize::new(0) }
    }
}

impl Stripes {
    fn stripe(&self, tx: TxId) -> Option<&Mutex<Vec<Holder>>> {
        usize::try_from(tx % STRIPES).ok().and_then(|at| self.stripes.get(at))
    }

    /// Takes a shared hold for `tx` here, unless the lock is blocked:
    /// whether it is new, or none if it was not taken.
    pub(super) fn try_shared(&self, tx: &Txn) -> Option<bool> {
        if self.blocked.load(Ordering::SeqCst) > 0 {
            return None;
        }
        let mut held = lock_builtin(self.stripe(tx.id)?);
        if self.blocked.load(Ordering::SeqCst) > 0 {
            return None;
        }
        if held.iter().any(|h| h.tx == tx.id) {
            return Some(false);
        }
        held.push(Holder { tx: tx.id, mode: LockMode::Shared, running: Arc::clone(&tx.running) });
        Some(true)
    }

    /// An exclusive request begins: no hold is taken here until it ends.
    pub(super) fn block(&self) {
        self.blocked.fetch_add(1, Ordering::SeqCst);
    }

    /// An exclusive request was refused, or its hold released.
    pub(super) fn unblock(&self) {
        self.blocked.fetch_sub(1, Ordering::SeqCst);
    }

    /// Whether an exclusive request or hold blocks the stripes.
    pub(super) fn blocked(&self) -> bool {
        self.blocked.load(Ordering::SeqCst) > 0
    }

    /// The transactions but `tx` holding the lock here.
    pub(super) fn others(&self, tx: TxId) -> Vec<Holder> {
        let mut others = Vec::new();
        for stripe in &self.stripes {
            let held = lock_builtin(stripe);
            others.extend(held.iter().filter(|h| h.tx != tx).map(|h| Holder { tx: h.tx, mode: h.mode, running: Arc::clone(&h.running) }));
        }
        others
    }

    /// Releases `tx`'s hold here, if it has one.
    pub(super) fn release(&self, tx: TxId) {
        if let Some(stripe) = self.stripe(tx) {
            lock_builtin(stripe).retain(|h| h.tx != tx);
        }
    }
}
