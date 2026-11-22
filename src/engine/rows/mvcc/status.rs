//! \[DESIGN\] The statuses of a store's writing transactions (RULES.md D29,
//! D440): which have committed, and when. A version a transaction wrote
//! names it until the version is stamped with its commit timestamp, and a
//! reader or writer that meets such a version asks here. So one store of
//! the commit timestamp makes every version of a commit committed for every
//! reader and writer of the store's tables, however many rows it wrote, and
//! the stamping can wait until the transaction's locks are released.
//!
//! A fixed array of slots, one chosen by the transaction's id, is read
//! without a lock. A transaction claims its slot before its first version
//! is written. A slot whose transaction committed and was stamped is free
//! to claim again only once no reader can still hold a version it read
//! before the stamping: every transaction then joined to a table of the
//! store has a snapshot at or before the slot's mark, the latest snapshot
//! the store was told of once the stamping ended, so a horizon past the
//! mark finds them all ended. A slot not free yet sends the transaction to
//! a map under a lock, which only a burst of more than `SLOTS` writing
//! transactions before the horizon moves ever uses.
//!
//! A transaction whose slot is taken again, or never was, is read as open.
//! Only a read outside any transaction, as a maintenance pass makes, can
//! still ask of one then; it reads the version as not committed, as before
//! the stamping, and reads it again (`RowTable::unpinned`).

use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::hash::xxhash::Xxh3Map;
use crate::sync::Alone;
use crate::txn::{Timestamp, TxId};

/// Slots of the array: transactions writing at once before one waits on
/// the map. A power of two, so ids next to each other fall half the array
/// apart, on separate cache lines.
const SLOTS: u64 = 16384;
/// Half of `SLOTS`.
const HALF: u64 = 8192;
/// An owner while a slot is being claimed: neither its old transaction
/// nor its new one.
const CLAIMING: TxId = TxId::MAX;

/// One transaction's status, in a slot of the array.
#[derive(Default)]
struct Slot {
    /// The transaction, 0 for none, or `CLAIMING`.
    owner: AtomicU64,
    /// Its commit timestamp, 0 while it is open.
    at: AtomicU64,
    /// The tables whose versions of it are not all stamped yet.
    pending: AtomicU64,
    /// Once none are, the latest snapshot then (`Statuses::seen`): 0 until.
    mark: AtomicU64,
}

/// One transaction's status, in the map.
#[derive(Debug, Default, Clone, Copy)]
struct Status {
    at: Timestamp,
    pending: u64,
    mark: Timestamp,
}

/// \[DESIGN\] The statuses of a store's writing transactions.
pub(in crate::engine::rows) struct Statuses {
    slots: Box<[Slot]>,
    overflow: RwLock<Xxh3Map<TxId, Status>>,
    /// How many the map holds, so an empty one is not locked.
    overflowing: AtomicUsize,
    /// The latest snapshot a transaction joined a table of the store at.
    /// Alone on its lines, as the horizon is, so raising either moves no
    /// line a commit reads the slots by (RULES.md D783).
    snapshots: Alone<AtomicU64>,
    /// The latest horizon a table of the store was collected at.
    horizon: Alone<AtomicU64>,
}

impl Default for Statuses {
    fn default() -> Statuses {
        Statuses {
            slots: (0..SLOTS).map(|_| Slot::default()).collect(),
            overflow: RwLock::default(),
            overflowing: AtomicUsize::new(0),
            snapshots: Alone(AtomicU64::new(0)),
            horizon: Alone(AtomicU64::new(0)),
        }
    }
}

/// Raises `atomic` to `value`, written only if it is higher, so a value
/// read by every transaction is seldom written.
fn raise(atomic: &AtomicU64, value: u64) {
    if value > atomic.load(Ordering::SeqCst) {
        atomic.fetch_max(value, Ordering::SeqCst);
    }
}

impl Statuses {
    fn slot(&self, tx: TxId) -> Option<&Slot> {
        let i = tx % SLOTS;
        let at = if i & 1_u64 == 0 { i >> 1_u32 } else { (i >> 1_u32) | HALF };
        usize::try_from(at).ok().and_then(|at| self.slots.get(at))
    }

    /// The slot of `tx`, if it holds it.
    fn held(&self, tx: TxId) -> Option<&Slot> {
        self.slot(tx).filter(|slot| slot.owner.load(Ordering::SeqCst) == tx)
    }

    fn overflow(&self) -> std::sync::RwLockWriteGuard<'_, Xxh3Map<TxId, Status>> {
        self.overflow.write().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A transaction joined a table of the store at `snapshot`, before it
    /// reads.
    pub(in crate::engine::rows) fn seen(&self, snapshot: Timestamp) {
        raise(&self.snapshots, snapshot);
    }

    /// A table of the store is collected at `horizon`: no transaction reads
    /// before it.
    pub(in crate::engine::rows) fn collected(&self, horizon: Timestamp) {
        raise(&self.horizon, horizon);
    }

    /// Whether a slot owned by `owner` may be claimed: by none, or by a
    /// transaction committed and stamped before every open snapshot.
    fn free(&self, slot: &Slot, owner: TxId) -> bool {
        if owner == 0 {
            return true;
        }
        // The count read before the mark, which is raised before the count
        // falls (`stamped`).
        if owner == CLAIMING || slot.pending.load(Ordering::SeqCst) != 0 {
            return false;
        }
        let mark = slot.mark.load(Ordering::SeqCst);
        mark != 0 && mark < self.horizon.load(Ordering::SeqCst)
    }

    /// Claims a status for `tx`, before its first version is written: its
    /// slot, or a place in the map. Claiming again does nothing.
    pub(in crate::engine::rows) fn claim(&self, tx: TxId) {
        let Some(slot) = self.slot(tx) else { return self.claim_overflow(tx) };
        let owner = slot.owner.load(Ordering::SeqCst);
        if owner == tx || self.in_overflow(tx) {
            return;
        }
        if !self.free(slot, owner) {
            return self.claim_overflow(tx);
        }
        let taken = slot.owner.compare_exchange(owner, CLAIMING, Ordering::SeqCst, Ordering::SeqCst).is_ok();
        if !taken {
            return self.claim_overflow(tx);
        }
        // Cleared before it is named, so its last transaction's status is
        // never read as this one's.
        slot.at.store(0, Ordering::SeqCst);
        slot.pending.store(0, Ordering::SeqCst);
        slot.mark.store(0, Ordering::SeqCst);
        slot.owner.store(tx, Ordering::SeqCst);
    }

    fn in_overflow(&self, tx: TxId) -> bool {
        self.overflowing.load(Ordering::SeqCst) > 0
            && self.overflow.read().unwrap_or_else(std::sync::PoisonError::into_inner).contains_key(&tx)
    }

    fn claim_overflow(&self, tx: TxId) {
        let mut overflow = self.overflow();
        overflow.entry(tx).or_default();
        self.overflowing.store(overflow.len(), Ordering::SeqCst);
    }

    /// The commit timestamp of `tx`, if it has committed; none while it is
    /// open, or once its status is forgotten.
    pub(in crate::engine::rows) fn committed(&self, tx: TxId) -> Option<Timestamp> {
        if let Some(slot) = self.held(tx) {
            let at = slot.at.load(Ordering::SeqCst);
            // Read again: a slot claimed meanwhile cleared it.
            if slot.owner.load(Ordering::SeqCst) == tx {
                return (at != 0).then_some(at);
            }
        }
        if self.overflowing.load(Ordering::SeqCst) == 0 {
            return None;
        }
        let overflow = self.overflow.read().unwrap_or_else(std::sync::PoisonError::into_inner);
        overflow.get(&tx).map(|status| status.at).filter(|&at| at != 0)
    }

    /// `tx` committed at `at` in a table, its versions there left to stamp
    /// if `rows`: the one store that makes them committed. A table with
    /// none left to stamp counts as stamped.
    /// A transaction that claimed no status, one that wrote nothing,
    /// writes nothing here (RULES.md D783).
    pub(in crate::engine::rows) fn commit(&self, tx: TxId, at: Timestamp, rows: bool) {
        let mark = || self.snapshots.load(Ordering::SeqCst).max(at);
        if let Some(slot) = self.held(tx) {
            if rows {
                slot.pending.fetch_add(1, Ordering::SeqCst);
                slot.mark.store(0, Ordering::SeqCst);
            } else {
                slot.mark.fetch_max(mark(), Ordering::SeqCst);
            }
            slot.at.store(at, Ordering::SeqCst);
            return;
        }
        if self.overflowing.load(Ordering::SeqCst) == 0 {
            return;
        }
        if let Some(status) = self.overflow().get_mut(&tx) {
            status.at = at;
            if rows {
                status.pending = status.pending.saturating_add(1);
                status.mark = 0;
            } else {
                status.mark = status.mark.max(mark());
            }
        }
    }

    /// `tx`, committed at `at`, has every version stamped in a table. The
    /// mark is the latest snapshot now, and never before `at`, so no slot
    /// is claimed again while a table of the commit is still to stamp.
    pub(in crate::engine::rows) fn stamped(&self, tx: TxId, at: Timestamp) {
        let mark = self.snapshots.load(Ordering::SeqCst).max(at);
        if let Some(slot) = self.held(tx) {
            // Marked before the count falls, so a count of none is read with
            // the mark.
            slot.mark.fetch_max(mark, Ordering::SeqCst);
            let mut pending = slot.pending.load(Ordering::SeqCst);
            while pending > 0 {
                match slot.pending.compare_exchange_weak(pending, pending.saturating_sub(1), Ordering::SeqCst, Ordering::SeqCst) {
                    Ok(_) => break,
                    Err(now) => pending = now,
                }
            }
            return;
        }
        if let Some(status) = self.overflow().get_mut(&tx) {
            status.mark = status.mark.max(mark);
            status.pending = status.pending.saturating_sub(1);
        }
    }

    /// `tx` rolled back: its versions are taken out or being taken out,
    /// read as open whether its status is kept or not, so it is forgotten.
    pub(in crate::engine::rows) fn abort(&self, tx: TxId) {
        if let Some(slot) = self.held(tx) {
            slot.at.store(0, Ordering::SeqCst);
            slot.pending.store(0, Ordering::SeqCst);
            slot.mark.store(0, Ordering::SeqCst);
            // Another claim cannot take a slot whose owner is open.
            let _ = slot.owner.compare_exchange(tx, 0, Ordering::SeqCst, Ordering::SeqCst);
            return;
        }
        if self.overflowing.load(Ordering::SeqCst) > 0 {
            let mut overflow = self.overflow();
            overflow.remove(&tx);
            self.overflowing.store(overflow.len(), Ordering::SeqCst);
        }
    }

    /// Forgets the transactions in the map a slot would now be free of.
    pub(in crate::engine::rows) fn sweep(&self) {
        if self.overflowing.load(Ordering::SeqCst) == 0 {
            return;
        }
        let horizon = self.horizon.load(Ordering::SeqCst);
        let mut overflow = self.overflow();
        overflow.retain(|_, s| s.pending != 0 || s.mark == 0 || s.mark >= horizon);
        self.overflowing.store(overflow.len(), Ordering::SeqCst);
    }
}

// Unit tests: claims, commits, reuse and the map (RULES.md R-TEST-1,
// R-TEST-8).
#[cfg(test)]
mod tests;
