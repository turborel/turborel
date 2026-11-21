//! \[DESIGN\] The database's logical clock (RULES.md D29): transaction ids,
//! commit timestamps, the stable timestamp snapshots read at, and the
//! horizon, the oldest open snapshot. Every transaction uses it, so no
//! global lock is taken on its way: transactions writing tables of their
//! own would otherwise wait on that one lock.
//!
//! * **Ids** come from one counter, unique; they order nothing.
//! * **Commit timestamps** come from another, so they are dense: 1, 2, 3,
//!   and so on. A commit that ends marks its timestamp in a ring; the
//!   *stable* timestamp, below which every commit has ended, is advanced
//!   over the marked ones by whichever commit ends, by compare-and-swap. A
//!   snapshot is one read of it. A commit waits only if the ring would
//!   wrap onto one not ended, with `RING` commits under way.
//! * **Durable** is the timestamp below which every commit has also been
//!   made durable, its commit record synced or needing no sync, kept as
//!   the stable timestamp is in a second ring (RULES.md D441). A snapshot
//!   is one read of it, so no reader sees a commit a crash could lose,
//!   and a commit replies once it reaches its timestamp. A failed sync
//!   halts it, as it does the database's changes.
//! * **Open snapshots** are counted in stripes, one chosen by the
//!   transaction's id, each under its own lock, with its oldest snapshot
//!   also in an atomic. The horizon is the durable timestamp, read first,
//!   then the least of the stripes' atomics, read without locks. A
//!   beginning transaction sets its stripe's atomic to 0 before it reads
//!   the durable timestamp, so a horizon read meanwhile sees 0 (and nothing
//!   is collected), or comes after its snapshot is counted. A transaction
//!   that moves to a new snapshot (READ COMMITTED) does so the same way.

use std::collections::BTreeMap;
use std::mem;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::thread::{self, Thread};
use std::time::Duration;

use super::{Timestamp, TxId};
use crate::sync::lock_builtin;

/// Commits that may be under way at once before one waits.
const RING: u64 = 4096;
/// Stripes of the open snapshots.
const STRIPES: u64 = 32;
/// A stripe with no snapshot.
const NONE: u64 = u64::MAX;
/// How long a wait for the durable timestamp sleeps before it looks again,
/// should a wake-up be missed: none is, as a waiter is counted before it
/// looks, so this only bounds a defect's cost.
const DURABLE_POLL: Duration = Duration::from_millis(10);

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
    /// The timestamp below which every commit is also durable, and for
    /// each modulo `RING` the last one made so.
    durable: AtomicU64,
    synced: Box<[AtomicU64]>,
    /// Whether a sync failed: the durable timestamp moves no more.
    halted: AtomicBool,
    /// The threads waiting for the durable timestamp, by the timestamp
    /// each waits for, and how many wait: each is woken alone, once the
    /// durable timestamp reaches its own (RULES.md D740).
    waits: Mutex<BTreeMap<Timestamp, Vec<Thread>>>,
    waiting: AtomicUsize,
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
            durable: AtomicU64::new(0),
            synced: (0..RING).map(|_| AtomicU64::new(0)).collect(),
            halted: AtomicBool::new(false),
            waits: Mutex::new(BTreeMap::new()),
            waiting: AtomicUsize::new(0),
            stripes: (0..STRIPES).map(|_| Stripe { snapshots: Mutex::new(BTreeMap::new()), oldest: AtomicU64::new(NONE) }).collect(),
        }
    }

    fn stripe(&self, id: TxId) -> Option<&Stripe> {
        usize::try_from(id % STRIPES).ok().and_then(|i| self.stripes.get(i))
    }

    fn ended(&self, at: Timestamp) -> Option<&AtomicU64> {
        usize::try_from(at % RING).ok().and_then(|i| self.ended.get(i))
    }

    fn synced(&self, at: Timestamp) -> Option<&AtomicU64> {
        usize::try_from(at % RING).ok().and_then(|i| self.synced.get(i))
    }

    /// A new transaction's id and snapshot, counted open.
    pub(super) fn begin(&self) -> (TxId, Timestamp) {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let Some(stripe) = self.stripe(id) else { return (id, self.durable.load(Ordering::SeqCst)) };
        let mut snapshots = lock_builtin(&stripe.snapshots);
        stripe.oldest.store(0, Ordering::SeqCst);
        let snapshot = self.durable.load(Ordering::SeqCst);
        let holders = snapshots.entry(snapshot).or_default();
        *holders = holders.saturating_add(1);
        stripe.oldest.store(snapshots.first_key_value().map_or(NONE, |(&s, _)| s), Ordering::SeqCst);
        (id, snapshot)
    }

    /// Transaction `id`, at snapshot `from`, reads from now at the durable
    /// timestamp, counted open in its place: its new snapshot.
    pub(super) fn moved(&self, id: TxId, from: Timestamp) -> Timestamp {
        let Some(stripe) = self.stripe(id) else { return self.durable.load(Ordering::SeqCst) };
        let mut snapshots = lock_builtin(&stripe.snapshots);
        stripe.oldest.store(0, Ordering::SeqCst);
        if let Some(holders) = snapshots.get_mut(&from) {
            *holders = holders.saturating_sub(1);
            if *holders == 0 {
                snapshots.remove(&from);
            }
        }
        let snapshot = self.durable.load(Ordering::SeqCst);
        let holders = snapshots.entry(snapshot).or_default();
        *holders = holders.saturating_add(1);
        stripe.oldest.store(snapshots.first_key_value().map_or(NONE, |(&s, _)| s), Ordering::SeqCst);
        snapshot
    }

    /// Transaction `id`, open at `snapshot`, counts it open once more, until
    /// as many `end`s of it: what it reads there is kept, though it moves
    /// on. Its count is already above 0, so the oldest is unchanged.
    pub(super) fn hold(&self, id: TxId, snapshot: Timestamp) {
        let Some(stripe) = self.stripe(id) else { return };
        let mut snapshots = lock_builtin(&stripe.snapshots);
        let holders = snapshots.entry(snapshot).or_default();
        *holders = holders.saturating_add(1);
        stripe.oldest.store(snapshots.first_key_value().map_or(NONE, |(&s, _)| s), Ordering::SeqCst);
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
        // Its slot in each ring is free once the commit `RING` before it is
        // durable; after a failed sync none is, and no change is taken.
        while at.saturating_sub(self.durable.load(Ordering::SeqCst)) >= RING && !self.halted.load(Ordering::SeqCst) {
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

    /// The commits at `ats`, ended, are durable: their commit records
    /// synced, or they need none. Each is marked, then the durable
    /// timestamp moves past those whose predecessors are all durable too,
    /// once for a group's sync, and the waiters it reaches are woken, each
    /// alone (RULES.md D740).
    pub(super) fn end_syncs(&self, ats: impl Iterator<Item = Timestamp>) {
        for at in ats {
            if let Some(slot) = self.synced(at) {
                slot.store(at, Ordering::SeqCst);
            }
        }
        let mut moved = false;
        loop {
            let durable = self.durable.load(Ordering::SeqCst);
            let next = durable.saturating_add(1);
            if self.synced(next).is_none_or(|slot| slot.load(Ordering::SeqCst) != next) {
                break;
            }
            moved |= self.durable.compare_exchange(durable, next, Ordering::SeqCst, Ordering::SeqCst).is_ok();
        }
        if moved && self.waiting.load(Ordering::SeqCst) > 0 {
            self.wake_through(self.durable.load(Ordering::SeqCst));
        }
    }

    /// Wakes the threads waiting for a timestamp at or before `durable`.
    fn wake_through(&self, durable: Timestamp) {
        let reached = {
            let mut waits = lock_builtin(&self.waits);
            let later = waits.split_off(&durable.saturating_add(1));
            mem::replace(&mut *waits, later)
        };
        let threads: Vec<Thread> = reached.into_values().flatten().collect();
        self.waiting.fetch_sub(threads.len(), Ordering::SeqCst);
        for thread in threads {
            thread.unpark();
        }
    }

    /// The timestamp below which every commit is durable.
    pub(super) fn durable(&self) -> Timestamp {
        self.durable.load(Ordering::SeqCst)
    }

    /// Returns once the durable timestamp has reached `at`: whether it has,
    /// or a failed sync halted it first. The thread waits on its own,
    /// counted before it looks again under the lock a waker takes, so a
    /// move of the durable timestamp between the look and the wait wakes it.
    pub(super) fn wait_durable(&self, at: Timestamp) -> bool {
        if self.durable.load(Ordering::SeqCst) >= at {
            return true;
        }
        if self.halted.load(Ordering::SeqCst) {
            return false;
        }
        {
            let mut waits = lock_builtin(&self.waits);
            self.waiting.fetch_add(1, Ordering::SeqCst);
            waits.entry(at).or_default().push(thread::current());
        }
        // Looked at after it is counted: a waker either sees it counted,
        // or moved the durable timestamp before this look.
        if self.durable.load(Ordering::SeqCst) >= at {
            self.wake_through(self.durable.load(Ordering::SeqCst));
        }
        loop {
            if self.durable.load(Ordering::SeqCst) >= at {
                return true;
            }
            if self.halted.load(Ordering::SeqCst) {
                return false;
            }
            thread::park_timeout(DURABLE_POLL);
        }
    }

    /// A sync of the log failed: the durable timestamp moves no more, and
    /// every wait for it ends.
    pub(super) fn halt(&self) {
        self.halted.store(true, Ordering::SeqCst);
        self.wake_through(Timestamp::MAX);
    }

    /// The oldest snapshot of an open transaction, or with none open, the
    /// durable timestamp: no transaction reads before it.
    pub(super) fn horizon(&self) -> Timestamp {
        let durable = self.durable.load(Ordering::SeqCst);
        self.stripes.iter().map(|s| s.oldest.load(Ordering::SeqCst)).fold(durable, u64::min)
    }

    /// Moves the counters past `highest`, an id or timestamp found in the
    /// log; with no commit under way, as in recovery, every commit up to it
    /// counts as ended.
    pub(super) fn advance_past(&self, highest: u64) {
        let next = highest.saturating_add(1);
        self.next_id.fetch_max(next, Ordering::SeqCst);
        let commit = self.next_commit.fetch_max(next, Ordering::SeqCst).max(next);
        self.stable.fetch_max(commit.saturating_sub(1), Ordering::SeqCst);
        self.durable.fetch_max(commit.saturating_sub(1), Ordering::SeqCst);
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

// Unit tests: the stable and durable timestamps and the horizon (RULES.md
// R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;

    /// Ends commit `at`, durable as it ends.
    fn ended(clock: &Clock, at: Timestamp) {
        clock.end_commit(at);
        clock.end_syncs(std::iter::once(at));
    }

    /// Verifies: [DESIGN] RULES.md D29, D441, the clock: commits ending out
    /// of order move the stable and durable timestamps only past those
    /// whose predecessors have all ended; snapshots read the durable one;
    /// the horizon is the oldest open snapshot, or with none, the durable
    /// timestamp; a ring's worth of commits wraps around.
    #[test]
    fn the_stable_timestamp_waits_for_earlier_commits() {
        let clock = Clock::new(1);
        let (a, b) = (clock.take_commit(), clock.take_commit());
        assert_eq!((a, b, clock.stable()), (1, 2, 0));
        ended(&clock, b);
        assert_eq!(clock.stable(), 0, "1 has not ended");
        let (reader, snapshot) = clock.begin();
        assert_eq!(snapshot, 0);
        ended(&clock, a);
        assert_eq!((clock.stable(), clock.durable()), (2, 2), "both ended");
        assert_eq!(clock.horizon(), 0, "the reader is still open");
        clock.end(reader, snapshot);
        assert_eq!(clock.horizon(), 2);
        for _ in 0..RING * 2 {
            let at = clock.take_commit();
            ended(&clock, at);
        }
        assert_eq!(clock.durable(), 2 + RING * 2);
        clock.advance_past(10 * RING);
        assert_eq!((clock.durable(), clock.take_commit()), (10 * RING, 10 * RING + 1));
        assert!(clock.next_id() > 10 * RING);
    }

    /// Verifies: [DESIGN] RULES.md D441: a commit ended but not yet synced
    /// is stable and not durable, so a snapshot does not count it; a wait
    /// for the durable timestamp ends once every commit before it is
    /// synced, or with a failed sync, which halts it.
    #[test]
    fn a_snapshot_waits_for_the_sync() {
        let clock = std::sync::Arc::new(Clock::new(1));
        let (a, b) = (clock.take_commit(), clock.take_commit());
        clock.end_commit(a);
        ended(&clock, b);
        assert_eq!((clock.stable(), clock.durable(), clock.begin().1), (2, 0, 0));
        let first = std::sync::Arc::clone(&clock);
        let waiter = thread::spawn(move || first.wait_durable(b));
        thread::sleep(Duration::from_millis(20));
        assert!(!waiter.is_finished(), "1 is not synced");
        clock.end_syncs(std::iter::once(a));
        assert!(waiter.join().unwrap());
        assert_eq!(clock.begin().1, 2);
        let c = clock.take_commit();
        clock.end_commit(c);
        let second = std::sync::Arc::clone(&clock);
        let halted = thread::spawn(move || second.wait_durable(c));
        clock.halt();
        assert!(!halted.join().unwrap(), "halted");
        assert_eq!(clock.durable(), 2);
    }
    /// Verifies: [DESIGN] RULES.md D441, D740: a waiter for the durable
    /// timestamp is woken alone, once it reaches the waiter's own: commits
    /// marked durable out of order wake no one until those before them are;
    /// one call marks several at once; a waiter for a later timestamp stays
    /// counted and asleep.
    #[test]
    fn a_waiter_is_woken_once_its_timestamp_is_durable() {
        let clock = std::sync::Arc::new(Clock::new(1));
        for _ in 0_u8..4 {
            let at = clock.take_commit();
            clock.end_commit(at);
        }
        let (early, late) = (std::sync::Arc::clone(&clock), std::sync::Arc::clone(&clock));
        let second = thread::spawn(move || early.wait_durable(2));
        let fourth = thread::spawn(move || late.wait_durable(4));
        while clock.waiting.load(Ordering::SeqCst) < 2 {
            thread::yield_now();
        }
        clock.end_syncs([2, 3].into_iter());
        assert_eq!((clock.durable(), clock.waiting.load(Ordering::SeqCst)), (0, 2), "1 is not durable");
        clock.end_syncs(std::iter::once(1));
        assert!(second.join().unwrap());
        assert_eq!((clock.durable(), clock.waiting.load(Ordering::SeqCst)), (3, 1), "4 waits on");
        assert!(!fourth.is_finished());
        clock.end_syncs(std::iter::once(4));
        assert!(fourth.join().unwrap());
        assert_eq!(clock.waiting.load(Ordering::SeqCst), 0);
    }
}
