//! \[DESIGN\] The database's logical clock (RULES.md D29): transaction ids,
//! commit timestamps, the stable timestamp snapshots read at, and the
//! horizon, the oldest open snapshot. Every transaction uses it, so no
//! global lock is taken on its way: a profile of eight threads writing
//! their own tables found them waiting, most of the time, on the one lock
//! this replaced.
//!
//! * **Ids** come from one counter, unique; they order nothing.
//! * **Commit timestamps** come from another, so they are dense: 1, 2, 3,
//!   and so on. A commit that ends marks its timestamp in a ring; the
//!   *stable* timestamp, below which every commit has ended, is advanced
//!   over the marked ones by whichever commit ends, by compare-and-swap. A
//!   snapshot is one read of it. A commit waits only if the ring would
//!   wrap onto one not ended, with `RING` commits under way.
//! * **Open snapshots** are counted in stripes, one chosen by the
//!   transaction's id, each under its own lock, with its oldest snapshot
//!   also in an atomic. The horizon is the stable timestamp, read first,
//!   then the least of the stripes' atomics, read without locks. A
//!   beginning transaction sets its stripe's atomic to 0 before it reads
//!   the stable timestamp, so a horizon read meanwhile sees 0 (and nothing
//!   is collected), or comes after its snapshot is counted. A transaction
//!   that moves to a new snapshot (READ COMMITTED) does so the same way.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

use super::{Timestamp, TxId};
use crate::sync::lock_builtin;

/// Commits that may be under way at once before one waits.
const RING: u64 = 4096;
/// Stripes of the open snapshots.
const STRIPES: u64 = 32;
/// A stripe with no snapshot.
const NONE: u64 = u64::MAX;

/// One stripe of the open snapshots, alone on its cache lines.
#[repr(align(128))]
struct Stripe {
    /// Each snapshot, with how many open transactions have it.
    snapshots: Mutex<BTreeMap<Timestamp, usize>>,
    /// The oldest of them, `NONE` for none, 0 while one is being added.
    oldest: AtomicU64,
}

/// \[DESIGN\] The logical clock.
pub(super) struct Clock {
    next_id: AtomicU64,
    next_commit: AtomicU64,
    stable: AtomicU64,
    /// For each commit timestamp modulo `RING`, the last one that ended.
    ended: Box<[AtomicU64]>,
    stripes: Box<[Stripe]>,
}

impl Clock {
    /// A clock whose first transaction id is `first`.
    pub(super) fn new(first: TxId) -> Clock {
        Clock {
            next_id: AtomicU64::new(first),
            next_commit: AtomicU64::new(1),
            stable: AtomicU64::new(0),
            ended: (0..RING).map(|_| AtomicU64::new(0)).collect(),
            stripes: (0..STRIPES).map(|_| Stripe { snapshots: Mutex::new(BTreeMap::new()), oldest: AtomicU64::new(NONE) }).collect(),
        }
    }

    fn stripe(&self, id: TxId) -> Option<&Stripe> {
        usize::try_from(id % STRIPES).ok().and_then(|i| self.stripes.get(i))
    }

    fn ended(&self, at: Timestamp) -> Option<&AtomicU64> {
        usize::try_from(at % RING).ok().and_then(|i| self.ended.get(i))
    }

    /// A new transaction's id and snapshot, counted open.
    pub(super) fn begin(&self) -> (TxId, Timestamp) {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let Some(stripe) = self.stripe(id) else { return (id, self.stable.load(Ordering::SeqCst)) };
        let mut snapshots = lock_builtin(&stripe.snapshots);
        stripe.oldest.store(0, Ordering::SeqCst);
        let snapshot = self.stable.load(Ordering::SeqCst);
        let holders = snapshots.entry(snapshot).or_default();
        *holders = holders.saturating_add(1);
        stripe.oldest.store(snapshots.first_key_value().map_or(NONE, |(&s, _)| s), Ordering::SeqCst);
        (id, snapshot)
    }

    /// Transaction `id`, at snapshot `from`, reads from now at the stable
    /// timestamp, counted open in its place: its new snapshot.
    pub(super) fn moved(&self, id: TxId, from: Timestamp) -> Timestamp {
        let Some(stripe) = self.stripe(id) else { return self.stable.load(Ordering::SeqCst) };
        let mut snapshots = lock_builtin(&stripe.snapshots);
        stripe.oldest.store(0, Ordering::SeqCst);
        if let Some(holders) = snapshots.get_mut(&from) {
            *holders = holders.saturating_sub(1);
            if *holders == 0 {
                snapshots.remove(&from);
            }
        }
        let snapshot = self.stable.load(Ordering::SeqCst);
        let holders = snapshots.entry(snapshot).or_default();
        *holders = holders.saturating_add(1);
        stripe.oldest.store(snapshots.first_key_value().map_or(NONE, |(&s, _)| s), Ordering::SeqCst);
        snapshot
    }

    /// Transaction `id`, at `snapshot`, has ended.
    pub(super) fn end(&self, id: TxId, snapshot: Timestamp) {
        let Some(stripe) = self.stripe(id) else { return };
        let mut snapshots = lock_builtin(&stripe.snapshots);
        if let Some(holders) = snapshots.get_mut(&snapshot) {
            *holders = holders.saturating_sub(1);
            if *holders == 0 {
                snapshots.remove(&snapshot);
            }
        }
        stripe.oldest.store(snapshots.first_key_value().map_or(NONE, |(&s, _)| s), Ordering::SeqCst);
    }

    /// A commit timestamp, later than every one given before.
    pub(super) fn take_commit(&self) -> Timestamp {
        let at = self.next_commit.fetch_add(1, Ordering::SeqCst);
        // Its slot in the ring is free once the commit `RING` before it ended.
        while at.saturating_sub(self.stable.load(Ordering::SeqCst)) >= RING {
            thread::yield_now();
        }
        at
    }

    /// The commit at `at` has ended, committed or not: the stable timestamp
    /// moves past it once every commit before it has ended too.
    pub(super) fn end_commit(&self, at: Timestamp) {
        if let Some(slot) = self.ended(at) {
            slot.store(at, Ordering::SeqCst);
        }
        loop {
            let stable = self.stable.load(Ordering::SeqCst);
            let next = stable.saturating_add(1);
            if self.ended(next).is_none_or(|slot| slot.load(Ordering::SeqCst) != next) {
                return;
            }
            // Another thread may have moved it already; either way, look again.
            let _ = self.stable.compare_exchange(stable, next, Ordering::SeqCst, Ordering::SeqCst);
        }
    }

    /// The highest timestamp below which every commit has ended.
    pub(super) fn stable(&self) -> Timestamp {
        self.stable.load(Ordering::SeqCst)
    }

    /// The oldest snapshot of an open transaction, or with none open, the
    /// stable timestamp: no transaction reads before it.
    pub(super) fn horizon(&self) -> Timestamp {
        let stable = self.stable.load(Ordering::SeqCst);
        self.stripes.iter().map(|s| s.oldest.load(Ordering::SeqCst)).fold(stable, u64::min)
    }

    /// Moves the counters past `highest`, an id or timestamp found in the
    /// log; with no commit under way, as in recovery, every commit up to it
    /// counts as ended.
    pub(super) fn advance_past(&self, highest: u64) {
        let next = highest.saturating_add(1);
        self.next_id.fetch_max(next, Ordering::SeqCst);
        let commit = self.next_commit.fetch_max(next, Ordering::SeqCst).max(next);
        self.stable.fetch_max(commit.saturating_sub(1), Ordering::SeqCst);
    }

    /// The highest transaction id or commit timestamp given so far, or past
    /// which the counters were moved.
    pub(super) fn highest(&self) -> u64 {
        self.next_id.load(Ordering::SeqCst).max(self.next_commit.load(Ordering::SeqCst)).saturating_sub(1)
    }

    /// The next transaction id: for tests.
    #[cfg(test)]
    pub(super) fn next_id(&self) -> TxId {
        self.next_id.load(Ordering::SeqCst)
    }
}

// Unit tests: the stable timestamp and the horizon (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D29, the clock: commits ending out of
    /// order move the stable timestamp only past those whose predecessors
    /// have all ended; snapshots read it; the horizon is the oldest open
    /// snapshot, or with none, the stable timestamp; a ring's worth of
    /// commits wraps around.
    #[test]
    fn the_stable_timestamp_waits_for_earlier_commits() {
        let clock = Clock::new(1);
        let (a, b) = (clock.take_commit(), clock.take_commit());
        assert_eq!((a, b, clock.stable()), (1, 2, 0));
        clock.end_commit(b);
        assert_eq!(clock.stable(), 0, "1 has not ended");
        let (reader, snapshot) = clock.begin();
        assert_eq!(snapshot, 0);
        clock.end_commit(a);
        assert_eq!(clock.stable(), 2, "both ended");
        assert_eq!(clock.horizon(), 0, "the reader is still open");
        clock.end(reader, snapshot);
        assert_eq!(clock.horizon(), 2);
        for _ in 0..RING * 2 {
            let at = clock.take_commit();
            clock.end_commit(at);
        }
        assert_eq!(clock.stable(), 2 + RING * 2);
        clock.advance_past(10 * RING);
        assert_eq!((clock.stable(), clock.take_commit()), (10 * RING, 10 * RING + 1));
        assert!(clock.next_id() > 10 * RING);
    }
}
