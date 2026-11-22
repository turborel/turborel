//! \[DESIGN\] Table locks: strict two-phase locking, so that concurrent
//! execution is serializable [X3.135-1986 §4.16]. A transaction that finds a
//! table locked against it is refused at once (`LockWait::NoWait`) or waits
//! until the holder ends (`LockWait::Wait`), unless waiting would deadlock or
//! could never end, which is refused at once. A wait ends refused past
//! `LOCKS.TIMEOUT_MS`, or when its statement is cancelled.
//!
//! Each table has a lock of its own (RULES.md D29), so transactions on
//! different tables never touch the same one. Only a transaction about to
//! wait takes the graph of who waits for whom; the lock order is the table's
//! lock first, then the graph.

use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, ThreadId};
use std::time::Instant;

mod fast;

use super::owed::OwedRemovals;
use super::{LockMode, LockWait, Timestamp, Transactions, TxId, Txn};
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::error::{Code, Result, SqlError};
use crate::sync::{Signal, lock, lock_builtin};

/// \[DESIGN\] How long a lock wait sleeps before it looks again for a
/// cancel of its statement (RULES.md D44), unless a release wakes it.
pub(super) const STOP_POLL: std::time::Duration = std::time::Duration::from_millis(20);

/// \[DESIGN\] What a failed undo damaged (RULES.md D19).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Damage {
    /// Index entries: the rows are as they should be, and the indexes,
    /// built from them again, are too (`Database::repair_table`).
    Indexes,
    /// Rows: what they should be is the last checkpoint's and the log's,
    /// which opening the database again recovers.
    Rows,
}

/// Index builds under way (`Database::create_index`), which checkpoints
/// wait for.
impl Transactions {
    /// How many transactions wait for others now.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the graph's lock is poisoned.
    pub(crate) fn waiting(&self) -> Result<usize> {
        Ok(lock(&self.waits_for)?.len())
    }

    /// Counts an index build under way, until `end_build`.
    pub(crate) fn begin_build(&self) {
        let mut builds = lock_builtin(&self.builds);
        *builds = builds.saturating_add(1);
    }

    /// An index build counted by `begin_build` ended.
    pub(crate) fn end_build(&self) {
        let mut builds = lock_builtin(&self.builds);
        *builds = builds.saturating_sub(1);
        self.built.notify_all();
    }

    /// Whether an index build is under way.
    pub(crate) fn building(&self) -> bool {
        *lock_builtin(&self.builds) > 0
    }

    /// Waits until no index build is under way.
    pub(crate) fn wait_for_builds(&self) {
        let mut builds = lock_builtin(&self.builds);
        while *builds > 0 {
            builds = self.built.wait(builds).unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

impl Txn {
    /// Records that it writes `table`, whose lock is `lock`, under the
    /// definitions of generation `generation`, the first time it does.
    ///
    /// # Errors
    ///
    /// SQLCODE -911 if the table's definitions changed after it first
    /// wrote it (`check_definitions`).
    pub(super) fn wrote_under(&self, table: &QualifiedName, lock: &Arc<TableLock>, generation: u64) -> Result<()> {
        let mut st = self.state()?;
        if !st.written.iter().any(|(t, ..)| t == table) {
            st.written.push((table.clone(), Arc::clone(lock), generation));
        }
        drop(st);
        self.check_definitions()
    }

    /// Records in each table it wrote that it committed at `at`.
    pub(crate) fn stamp_written(&self, at: i64) {
        if let Ok(st) = self.state() {
            st.written.iter().for_each(|(_, lock, _)| lock.stamp(at, false));
        }
    }

    /// The tables it wrote, in the order it first wrote each.
    pub(crate) fn written_tables(&self) -> Vec<QualifiedName> {
        self.state().map(|st| st.written.iter().map(|(t, ..)| t.clone()).collect()).unwrap_or_default()
    }

    /// Records that it changed definitions (`db/ddl.rs`): it then commits
    /// through its commit record, as a transaction that wrote does.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if its state's lock is poisoned.
    pub(crate) fn wrote_definitions(&self) -> Result<()> {
        self.state()?.wrote = true;
        Ok(())
    }

    /// Whether it may commit: not once a statement of it could not be
    /// undone, or its undoing could not be logged (RULES.md D19, D25).
    ///
    /// # Errors
    ///
    /// SQLCODE -913, the transaction to be rolled back.
    pub(crate) fn check_undone(&self) -> Result<()> {
        if let Some(why) = self.state()?.doomed {
            return Err(SqlError::new(Code::CommitFailed, format!("[X3.135-1986 §7.3 GR3b i] {why}, so the transaction was rolled back")));
        }
        Ok(())
    }

    /// \[DESIGN\] Whether it may go on: not if a table it wrote has had an
    /// index added or dropped since (`Database::create_index`,
    /// `db/indexes.rs`). An index added has no entry for what it wrote
    /// before, so it must not commit, nor read through the index, and can
    /// only roll back and run again.
    ///
    /// # Errors
    ///
    /// SQLCODE -911, naming the table.
    pub(crate) fn check_definitions(&self) -> Result<()> {
        let st = self.state()?;
        match st.written.iter().find(|(_, lock, generation)| lock.changed_after(*generation)) {
            None => Ok(()),
            Some((table, ..)) => Err(SqlError::new(
                Code::Conflict,
                format!("[DESIGN] an index was added to {table} after this transaction wrote it: roll back and run the transaction again"),
            )),
        }
    }
}

/// The rule every refusal cites: the statement is undone, and the
/// transaction goes on [X3.135-1986 §7.3 GR3b].
const UNDONE: &str = "the statement is rejected and undone [X3.135-1986 §7.3 GR3b]";

/// One transaction holding a table's lock.
struct Holder {
    tx: TxId,
    mode: LockMode,
    /// The thread the holder's running call is on, if one is (`Txn::running`).
    running: Arc<Mutex<Option<ThreadId>>>,
}

/// A table's collection under way (`TableLock::try_collect`), until dropped.
pub(crate) struct Collecting<'l>(&'l TableLock);

impl Drop for Collecting<'_> {
    fn drop(&mut self) {
        self.0.collecting.store(false, Ordering::Release);
    }
}

/// \[DESIGN\] What the transactions share about one table: who holds its
/// lock, the signal that a holder released it, whether a failed undo damaged
/// it (RULES.md D19), the snapshot that last changed its definitions
/// (RULES.md D27), and when its rows were last changed and compacted
/// (RULES.md D522). Every snapshot that knows the table shares it.
#[derive(Default)]
pub(crate) struct TableLock {
    holders: Mutex<Vec<Holder>>,
    /// How many `holders` lists, so a release finds an empty list unlocked.
    listed: AtomicUsize,
    /// Shared holds taken in stripes (RULES.md D442).
    fast: fast::Stripes,
    released: Signal,
    /// Set when a change to the table could not be undone, until
    /// `Database::repair_table` builds its indexes again, or opening the
    /// database again recovers its rows (RULES.md D19). The damage a failed
    /// undo did it (`Damage`), 0 for none.
    damaged: AtomicU8,
    /// The generation of the snapshot that last changed the table's
    /// definitions (an index added or dropped), or 0.
    changed_at: AtomicU64,
    /// Holds on collection of its garbage: by an index being built
    /// (`Database::create_index`), which counts every version it finds, and
    /// by a checkpoint logging the entries of what awaits collection
    /// (`db/checkpoint.rs`).
    holds: AtomicUsize,
    /// The generation of its storage last published by ALTER TABLE, and the
    /// commit timestamp it was published at (`db/ddl.rs`).
    generation: AtomicU64,
    replaced_at: AtomicU64,
    /// The index entries collection could not take back, tried again by
    /// its next (`txn/owed.rs`).
    pub(crate) owed: OwedRemovals,
    /// Of each index of it filled from its versions, by name, the latest
    /// commit the fill may have counted (`txn/kept.rs`).
    filled: Mutex<Vec<(String, Timestamp)>>,
    /// Whether a transaction's end is collecting its garbage now.
    collecting: AtomicBool,
    /// Whether it is in the set of tables awaiting collection, kept with
    /// the set under its lock, so an end that would leave it so changes
    /// nothing shared (RULES.md D795).
    noted: AtomicBool,
    /// When a transaction that wrote it last committed, and when its
    /// engine's maintenance last did some work, as a timestamp's instant;
    /// 0 for never (TABLE_STATISTICS, RULES.md D522).
    changed: AtomicI64,
    compacted: AtomicI64,
}

impl TableLock {
    /// Whether it is in the set of tables awaiting collection.
    pub(crate) fn noted(&self) -> bool {
        self.noted.load(Ordering::Acquire)
    }

    /// Notes whether it is in the set of tables awaiting collection, under
    /// the set's lock.
    pub(crate) fn set_noted(&self, noted: bool) {
        self.noted.store(noted, Ordering::Release);
    }

    /// Whether a failed undo damaged the table.
    pub(crate) fn is_damaged(&self) -> bool {
        self.damage().is_some()
    }

    /// The damage a failed undo did the table, if one did.
    pub(crate) fn damage(&self) -> Option<Damage> {
        match self.damaged.load(Ordering::SeqCst) {
            0 => None,
            1 => Some(Damage::Indexes),
            _ => Some(Damage::Rows),
        }
    }

    /// Marks the table damaged: in its rows, if `rows`, else in its
    /// indexes alone; damage to the rows stays so.
    pub(super) fn damaged(&self, rows: bool) {
        self.damaged.fetch_max(if rows { 2 } else { 1 }, Ordering::SeqCst);
    }

    /// Records that a transaction that wrote it committed at `at`, or that
    /// its engine's maintenance did some work at `at` if `compacted`: a
    /// relaxed store, as the last one stored is near enough the latest,
    /// made only when the instant moves (RULES.md D795).
    pub(crate) fn stamp(&self, at: i64, compacted: bool) {
        let stamped = if compacted { &self.compacted } else { &self.changed };
        if stamped.load(Ordering::Relaxed) != at {
            stamped.store(at, Ordering::Relaxed);
        }
    }

    /// When a transaction that wrote it last committed, and when its
    /// maintenance last did some work, each if it ever did.
    pub(crate) fn stamps(&self) -> (Option<i64>, Option<i64>) {
        let at = |a: &AtomicI64| Some(a.load(Ordering::Relaxed)).filter(|&t| t != 0);
        (at(&self.changed), at(&self.compacted))
    }

    /// Marks the table repaired (`Database::repair_table`).
    pub(crate) fn repaired(&self) {
        self.damaged.store(0, Ordering::SeqCst);
    }

    /// Records that snapshot `generation` changed the table's definitions.
    pub(crate) fn definitions_changed(&self, generation: u64) {
        self.changed_at.fetch_max(generation, Ordering::SeqCst);
    }

    /// Whether the table's definitions changed after snapshot `generation`.
    pub(crate) fn changed_after(&self, generation: u64) -> bool {
        self.changed_at.load(Ordering::SeqCst) > generation
    }

    /// Records that ALTER TABLE or TRUNCATE TABLE published generation
    /// `generation` of its storage, by a transaction that committed at `at`.
    pub(crate) fn replaced(&self, generation: u32, at: Timestamp) {
        self.generation.fetch_max(u64::from(generation), Ordering::SeqCst);
        self.replaced_at.fetch_max(at, Ordering::SeqCst);
    }

    /// \[DESIGN\] Whether a statement whose definitions have generation
    /// `generation` of the table, in transaction `tx`, may read or write
    /// it: not if a later one was published, nor if the transaction began
    /// before it was; its snapshot does not see the rows copied there.
    ///
    /// # Errors
    ///
    /// SQLCODE -911, naming `table`.
    pub(crate) fn check_generation(&self, table: &QualifiedName, generation: u32, tx: &Txn) -> Result<()> {
        if u64::from(generation) < self.generation.load(Ordering::SeqCst) {
            return Err(refused(&format!("{table} was altered or truncated after the statement began; run it again")));
        }
        if tx.snapshot() < self.replaced_at.load(Ordering::SeqCst) {
            return Err(SqlError::new(
                Code::Conflict,
                format!(
                    "[DESIGN] {table} was altered, truncated or defined after this transaction began: roll back and run the transaction again"
                ),
            ));
        }
        Ok(())
    }

    /// Whether collection of its garbage is held back.
    pub(crate) fn collection_held(&self) -> bool {
        self.holds.load(Ordering::SeqCst) > 0
    }

    /// Notes that the index `name` is filled from the table's versions
    /// committed through `through` (RULES.md D443): it counts each of
    /// those, so none superseded by then shares an entry with its successor.
    pub(crate) fn filled(&self, name: &str, through: Timestamp) {
        let mut filled = lock_builtin(&self.filled);
        filled.retain(|(n, _)| n != name);
        filled.push((name.to_owned(), through));
    }

    /// The latest commit a fill of the index `name` may have counted, or 0
    /// for an index not filled from versions while the table had any.
    pub(crate) fn filled_through(&self, name: &str) -> Timestamp {
        lock_builtin(&self.filled).iter().find(|(n, _)| n == name).map_or(0, |&(_, at)| at)
    }

    /// The table's collection, for one transaction's end at a time
    /// (RULES.md D445): none while another's is under way, or, if `wait`,
    /// once it is done.
    pub(crate) fn try_collect(&self, wait: bool) -> Option<Collecting<'_>> {
        loop {
            if !self.collecting.swap(true, Ordering::AcqRel) {
                return Some(Collecting(self));
            }
            if !wait {
                return None;
            }
            thread::yield_now();
        }
    }

    /// Holds collection of its garbage back, until `release_collection`.
    pub(crate) fn hold_collection(&self) {
        self.holds.fetch_add(1, Ordering::SeqCst);
    }

    /// A hold `hold_collection` took is given up.
    pub(crate) fn release_collection(&self) {
        self.holds.fetch_sub(1, Ordering::SeqCst);
    }

    /// Releases `tx`'s hold, and wakes every transaction waiting for the
    /// table so it can try again. For `Txn`'s `drop`, which cannot report an
    /// error: the lock is never held across plug-in code, so no panic can
    /// poison it.
    pub(super) fn release(&self, tx: TxId) {
        self.fast.release(tx);
        // Its holds all in the stripes, and no exclusive request to wake.
        if self.listed.load(Ordering::SeqCst) == 0 && !self.fast.blocked() {
            return;
        }
        let mut holders = lock_builtin(&self.holders);
        if holders.iter().any(|h| h.tx == tx && h.mode == LockMode::Exclusive) {
            self.fast.unblock();
        }
        holders.retain(|h| h.tx != tx);
        self.listed.store(holders.len(), Ordering::SeqCst);
        drop(holders);
        self.released.notify_all();
    }

    /// An exclusive request of `tx` begins: the stripes take no shared hold
    /// until it ends, unless `tx` holds the lock exclusively already.
    /// Whether it blocked them.
    fn block_for(&self, tx: TxId) -> Result<bool> {
        let holders = lock(&self.holders)?;
        if holders.iter().any(|h| h.tx == tx && h.mode == LockMode::Exclusive) {
            return Ok(false);
        }
        self.fast.block();
        Ok(true)
    }
}

/// The transactions that wait for others: for each, those it waits for.
pub(super) type WaitsFor = HashMap<TxId, Vec<TxId>>;

/// Whether `tx` waiting for `holders` would close a cycle of waiting
/// transactions back to `tx`: a deadlock. Searched without recursion, each
/// transaction once (RULES.md R-CF-6).
pub(super) fn would_deadlock(waits_for: &WaitsFor, tx: TxId, holders: &[TxId]) -> bool {
    let mut seen: HashSet<TxId> = HashSet::new();
    let mut next: Vec<TxId> = holders.to_vec();
    while let Some(t) = next.pop() {
        if t == tx {
            return true;
        }
        if seen.insert(t) {
            next.extend(waits_for.get(&t).into_iter().flatten().copied());
        }
    }
    false
}

impl LockMode {
    /// Whether two transactions may hold the lock in `self` and `other`
    /// together.
    fn compatible(self, other: LockMode) -> bool {
        matches!((self, other), (LockMode::Shared, LockMode::Shared | LockMode::Update) | (LockMode::Update, LockMode::Shared))
    }
}

/// The refusal of a lock request, giving `why`.
fn refused(why: &str) -> SqlError {
    SqlError::new(Code::Conflict, format!("[DESIGN] {why}, so {UNDONE}"))
}

impl State {
    /// Takes a lock on `table` in `mode` for `tx`, waiting while another
    /// transaction holds a conflicting one if the setting is `LockWait::Wait`.
    /// Shared locks are compatible with each other and with an update lock;
    /// an exclusive lock with none (`LockMode::compatible`). A held lock is
    /// only ever strengthened.
    ///
    /// # Errors
    ///
    /// SQLCODE -911 if the table is locked and the setting is no-wait, or if
    /// waiting would deadlock, could never end, or lasts past `LOCKS.TIMEOUT_MS`;
    /// SQLCODE -920 if its statement is cancelled or past its deadline
    /// (`Stop::check_now`); SQLCODE -910 if a lock is
    /// poisoned.
    pub(crate) fn acquire(&self, tx: &Txn, table: &QualifiedName, mode: LockMode) -> Result<()> {
        acquire_lock(tx, table, self.lock_of(table)?, mode)
    }

    /// Takes a lock on the name `name` in `mode` for `tx`, as `acquire`
    /// does: its table's lock, if it is a table's; else the name's own
    /// (`Transactions::name_lock`), which a table defined under it takes.
    ///
    /// # Errors
    ///
    /// As `acquire`'s.
    pub(crate) fn acquire_name(&self, tx: &Txn, name: &QualifiedName, mode: LockMode) -> Result<()> {
        let lk = self.locks.get(name).cloned().unwrap_or_else(|| self.tx.name_lock(name));
        acquire_lock(tx, name, &lk, mode)
    }
}

/// Takes `lk`, the lock of `table`, in `mode` for `tx` (`State::acquire`).
fn acquire_lock(tx: &Txn, table: &QualifiedName, lk: &Arc<TableLock>, mode: LockMode) -> Result<()> {
    // [DESIGN] RULES.md D442: a shared hold in a stripe, while no exclusive
    // request blocks them.
    let fast = if mode == LockMode::Shared { lk.fast.try_shared(tx) } else { None };
    if let Some(newly) = fast {
        if newly {
            lock_builtin(&tx.state).held.push(Arc::clone(lk));
        }
        return Ok(());
    }
    let blocks = mode == LockMode::Exclusive && lk.block_for(tx.id)?;
    let (outcome, mut holders) = wait_on(tx, table, lk, (&|h| h.tx != tx.id && !mode.compatible(h.mode), mode == LockMode::Exclusive))?;
    let newly = outcome.is_ok() && grant(&mut holders, tx, mode);
    lk.listed.store(holders.len(), Ordering::SeqCst);
    if outcome.is_err() && blocks {
        lk.fast.unblock();
    }
    drop(holders);
    outcome?;
    // Recorded even if a panic poisoned the transaction's lock, so that
    // ending the transaction always releases what it was granted.
    if newly {
        lock_builtin(&tx.state).held.push(Arc::clone(lk));
    }
    Ok(())
}

/// The holders of `lk` once none `conflicts` with `tx`, nor, if `striped`,
/// holds it in a stripe, or the refusal of the wait, `what` named in it.
/// Its lock held still, `tx` no longer among those waiting.
///
/// # Errors
///
/// SQLCODE -910 if a lock is poisoned.
fn wait_on<'l>(
    tx: &Txn,
    what: &dyn Display,
    lk: &'l TableLock,
    (conflicts, striped): (&dyn Fn(&Holder) -> bool, bool),
) -> Result<(Result<()>, MutexGuard<'l, Vec<Holder>>)> {
    let me = thread::current().id();
    let mut holders = lock(&lk.holders)?;
    let mut waited = false;
    let began = Instant::now();
    let outcome = loop {
        // A statement cancelled as it waits stops waiting (RULES.md D44).
        if let Err(e) = tx.stop.check_now() {
            break Err(e);
        }
        let fast = if striped { lk.fast.others(tx.id) } else { Vec::new() };
        let conflicting: Vec<&Holder> = holders.iter().filter(|h| conflicts(h)).chain(fast.iter()).collect();
        if conflicting.is_empty() {
            break Ok(());
        }
        if tx.lock_wait == LockWait::NoWait {
            break Err(refused(&format!("{what} is locked by another transaction; locking is no-wait")));
        }
        // A holder running a call on this very thread (a plug-in called
        // back) could never release the lock.
        if conflicting.iter().any(|h| *lock_builtin(&h.running) == Some(me)) {
            break Err(refused(&format!("{what} is locked by a transaction running on this thread, which could never release it")));
        }
        let ids: Vec<TxId> = conflicting.iter().map(|h| h.tx).collect();
        let mut waits_for = lock(&tx.shared.waits_for)?;
        if would_deadlock(&waits_for, tx.id, &ids) {
            break Err(refused(&format!(
                "waiting for {what} would deadlock with another transaction (roll back to release this transaction's locks)"
            )));
        }
        if let Some(most) = tx.stop.lock_timeout().filter(|&most| began.elapsed() >= most) {
            break Err(refused(&format!("{what} stayed locked by another transaction past LOCKS.TIMEOUT_MS, {} ms", most.as_millis())));
        }
        if !waited {
            // [DESIGN] RULES.md D520: shown waiting, and for whom.
            tx.stop.activity().waits_for_lock(what.to_string(), ids.first().copied().unwrap_or(0));
        }
        waits_for.insert(tx.id, ids);
        drop(waits_for);
        waited = true;
        // Woken by a release, or now and then to look for a cancel or a
        // timeout.
        holders = lk.released.wait_timeout(holders, STOP_POLL).map_err(|_| poisoned_wait())?.0;
    };
    if waited {
        tx.stop.activity().woke();
        lock(&tx.shared.waits_for)?.remove(&tx.id);
        // [DESIGN] RULES.md D240: the time waited, for the client.
        tx.stop.served().waited(began.elapsed());
    }
    Ok((outcome, holders))
}

/// Records `tx` holding the lock in `mode`, and whether it did not hold it before.
fn grant(holders: &mut Vec<Holder>, tx: &Txn, mode: LockMode) -> bool {
    if let Some(held) = holders.iter_mut().find(|h| h.tx == tx.id) {
        held.mode = held.mode.max(mode);
        return false;
    }
    holders.push(Holder { tx: tx.id, mode, running: Arc::clone(&tx.running) });
    true
}

/// The error for a wait whose lock a panicking thread poisoned.
fn poisoned_wait() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] a thread panicked while it held a table's lock (RULES.md D27)")
}
