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

use std::fmt::Display;
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::ThreadId;

use super::owed::OwedRemovals;
use super::row_locks::RowLocks;
use super::waiting::{LockRow, Sought, Waiter, Waits, refused, runs_here, wait_for};
use super::{LockMode, MODES, Modes, Timestamp, Transactions, TxId, Txn};
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::error::{Code, Result, SqlError};
use crate::sync::{Signal, lock, lock_builtin};

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
    /// The lock of the name `name`, made if none is held.
    pub(crate) fn name_lock(&self, name: &QualifiedName) -> Arc<TableLock> {
        let mut names = lock_builtin(&self.names);
        if let Some(held) = names.get(name).and_then(std::sync::Weak::upgrade) {
            return held;
        }
        names.retain(|_, weak| weak.strong_count() > 0);
        let made = Arc::new(TableLock::default());
        names.insert(name.clone(), Arc::downgrade(&made));
        made
    }

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

/// One transaction holding a table's lock.
struct Holder {
    tx: TxId,
    /// The modes it holds the lock in.
    modes: Modes,
    /// Its session (`Stop::session`), as INFORMATION_SCHEMA.LOCKS shows it.
    session: u64,
    /// The thread the holder's running call is on, if one is (`Txn::running`).
    running: Arc<Mutex<Option<ThreadId>>>,
}

/// \[DESIGN\] What the transactions share about one table: who holds its
/// lock, the signal that a holder released it, whether a failed undo damaged
/// it (RULES.md D19), and the snapshot that last changed its definitions
/// (RULES.md D27). Every snapshot that knows the table shares it.
#[derive(Default)]
pub(crate) struct TableLock {
    holders: Mutex<Vec<Holder>>,
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
    /// \[EXTENSION\] The locks of its rows (RULES.md D185).
    pub(crate) rows: RowLocks,
}

impl TableLock {
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
        lock_builtin(&self.holders).retain(|h| h.tx != tx);
        self.released.notify_all();
        self.rows.release(tx);
    }

    /// \[EXTENSION\] The locks held of `table`, whose lock it is, and of its
    /// rows: a row for each mode each transaction holds (RULES.md D188).
    pub(crate) fn held_locks(&self, table: &QualifiedName) -> Vec<LockRow> {
        let holders = lock_builtin(&self.holders);
        let mut rows: Vec<LockRow> = holders
            .iter()
            .flat_map(|h| {
                MODES.into_iter().filter(|m| h.modes & m.bit() != 0).map(|m| LockRow {
                    lock: Sought::Table(table.clone(), m),
                    session: h.session,
                    tx: Some(h.tx),
                    waiting: false,
                })
            })
            .collect();
        drop(holders);
        rows.extend(self.rows.held_locks(table));
        rows
    }
}

impl Txn {
    /// \[EXTENSION\] Records that it may hold locks of rows of the table
    /// whose lock is `lk`, so its end releases them (RULES.md D185).
    pub(crate) fn hold_rows(&self, lk: &Arc<TableLock>) {
        let mut st = lock_builtin(&self.state);
        if !st.held.iter().any(|held| Arc::ptr_eq(held, lk)) {
            st.held.push(Arc::clone(lk));
        }
    }
}

impl State {
    /// Takes a lock on `table` in `mode` for `tx`, waiting while another
    /// transaction holds a conflicting one if the setting is `LockWait::Wait`
    /// (`LockMode::conflicts_with`). A lock held in a mode is held in it to
    /// the transaction's end.
    ///
    /// # Errors
    ///
    /// SQLCODE -911 if the table is locked and the setting is no-wait, or if
    /// waiting would deadlock, could never end, or lasts past `LOCKS.TIMEOUT_MS`;
    /// SQLCODE -920 if its statement is cancelled or past its deadline
    /// (`Stop::check_now`); SQLCODE -910 if a lock is
    /// poisoned.
    pub(crate) fn acquire(&self, tx: &Txn, table: &QualifiedName, mode: LockMode) -> Result<()> {
        acquire_lock((tx, Waits::Setting), table, self.lock_of(table)?, mode)
    }

    /// \[EXTENSION\] As `acquire`, waiting as `waits` says: of LOCK TABLE
    /// (RULES.md D186).
    ///
    /// # Errors
    ///
    /// As `acquire`'s; of NOWAIT, SQLSTATE TBL01 if the table is locked.
    pub(crate) fn acquire_waiting(&self, tx: &Txn, table: &QualifiedName, mode: LockMode, waits: Waits) -> Result<()> {
        acquire_lock((tx, waits), table, self.lock_of(table)?, mode)
    }

    /// \[EXTENSION\] As `acquire_waiting`, false if `waits` is `Waits::Skip`
    /// and another transaction holds the lock against `mode`: of SKIP
    /// LOCKED of a table locked whole (RULES.md D185).
    ///
    /// # Errors
    ///
    /// As `acquire_waiting`'s.
    pub(crate) fn try_acquire(&self, tx: &Txn, table: &QualifiedName, mode: LockMode, waits: Waits) -> Result<bool> {
        take_lock((tx, waits), table, self.lock_of(table)?, mode)
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
        acquire_lock((tx, Waits::Setting), name, &lk, mode)
    }
}

/// Takes `lk`, the lock of `table`, in `mode` for `tx` (`State::acquire`).
fn acquire_lock((tx, waits): (&Txn, Waits), table: &QualifiedName, lk: &Arc<TableLock>, mode: LockMode) -> Result<()> {
    if take_lock((tx, waits), table, lk, mode)? {
        return Ok(());
    }
    Err(refused(&format!("{table} is locked by another transaction")))
}

/// As `acquire_lock`, false if `waits` is `Waits::Skip` and the lock is
/// held against `mode` (`State::try_acquire`).
fn take_lock((tx, waits): (&Txn, Waits), table: &QualifiedName, lk: &Arc<TableLock>, mode: LockMode) -> Result<bool> {
    let sought = Sought::Table(table.clone(), mode);
    let (outcome, mut holders) = wait_on((&tx.waiter(), waits), (table, &sought), lk, &|h| h.tx != tx.id && mode.conflicts_with(h.modes))?;
    let newly = matches!(outcome, Ok(true)) && grant(&mut holders, tx, mode);
    drop(holders);
    if !outcome? {
        return Ok(false);
    }
    // Recorded even if a panic poisoned the transaction's lock, so that
    // ending the transaction always releases what it was granted.
    if newly {
        lock_builtin(&tx.state).held.push(Arc::clone(lk));
    }
    Ok(true)
}

/// The holders of `lk` once none `conflicts` with `tx`, or the refusal of
/// the wait, `what` named in it: a table, or a row of one, the lock
/// `sought`. Ok(false) of SKIP LOCKED with a holder there. Its lock held
/// still, `tx` no longer among those waiting (`waiting::wait_for`).
///
/// # Errors
///
/// SQLCODE -910 if a lock is poisoned.
fn wait_on<'l>(
    (who, waits): (&Waiter, Waits),
    (what, sought): (&dyn Display, &Sought),
    lk: &'l TableLock,
    conflicts: &dyn Fn(&Holder) -> bool,
) -> Result<(Result<bool>, MutexGuard<'l, Vec<Holder>>)> {
    let holding = |holders: &Vec<Holder>, me: ThreadId| -> Vec<(TxId, bool)> {
        holders.iter().filter(|h| conflicts(h)).map(|h| (h.tx, runs_here(&h.running, me))).collect()
    };
    wait_for((who, waits), (what, sought), (&lk.holders, &lk.released), &holding)
}

impl State {
    /// [ISO/IEC 9075:1992 §4.28] Waits, as `acquire` waits, for transaction
    /// `writer`, which changed a row of `table` that `tx` would change, to
    /// end: every writer of a table whose engine keeps versions holds its
    /// lock shared to its end (`prepare_write`), so its end is a release of
    /// the lock (RULES.md D28, D29).
    ///
    /// # Errors
    ///
    /// As `acquire`'s, the refusal naming a row of `table`.
    pub(crate) fn wait_for_writer(&self, tx: &Txn, table: &QualifiedName, writer: TxId) -> Result<()> {
        let sought = Sought::Row(table.clone(), None, true);
        self.lock_of(table)?.wait_for_writer(&tx.waiter(), (&format_args!("a row of {table}"), &sought), writer)
    }
}

impl TableLock {
    /// As `State::wait_for_writer`, of `who`, the row named `what`, whose
    /// lock it seeks as `sought`.
    ///
    /// # Errors
    ///
    /// As `State::wait_for_writer`.
    pub(crate) fn wait_for_writer(&self, who: &Waiter, (what, sought): (&dyn Display, &Sought), writer: TxId) -> Result<()> {
        let (outcome, holders) = wait_on((who, Waits::Setting), (what, sought), self, &|h| h.tx == writer)?;
        drop(holders);
        outcome.map(|_| ())
    }
}

/// Records `tx` holding the lock in `mode`, and whether it did not hold it before.
fn grant(holders: &mut Vec<Holder>, tx: &Txn, mode: LockMode) -> bool {
    if let Some(held) = holders.iter_mut().find(|h| h.tx == tx.id) {
        held.modes |= mode.bit();
        return false;
    }
    holders.push(Holder { tx: tx.id, modes: mode.bit(), session: tx.stop.session, running: Arc::clone(&tx.running) });
    true
}
