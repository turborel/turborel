//! The transaction coordinator: it begins transactions, locks tables, keeps
//! the undo log, and commits or rolls back across storage engines.
//!
//! What the standard requires [X3.135-1986 §4.16]:
//! * a transaction is "atomic with respect to recovery and concurrency";
//! * ROLLBACK cancels all its changes, COMMIT makes them permanent
//!   [X3.135-1986 §8.2 GR3, §8.9 GR1];
//! * concurrent execution is serializable, and a transaction's changes are
//!   invisible to others until it commits;
//! * a statement that fails has its changes cancelled, while the transaction
//!   continues [X3.135-1986 §7.3 GR3b i];
//! * a transaction starts with the first statement when none is active
//!   [X3.135-1986 §7.3 GR2a].
//!
//! \[DESIGN\] How these hold depends on the storage, so the work is split.
//! Each storage engine declares `TxCapabilities`; the coordinator here owns
//! the lifecycle (ids, participants, savepoints, commit protocol) and supplies
//! defaults so that a simple engine gets correct behaviour for free:
//!
//! * `Undo::Log`: the coordinator keeps an undo log of inverse operations.
//!   `Undo::Native`: the engine keeps its own, via `TableStorage::savepoint`,
//!   `rollback_to` and `rollback`.
//! * `Concurrency::Coordinator`: the coordinator takes table locks held to the
//!   end of the transaction (strict two-phase locking, which gives
//!   serializable execution). `Concurrency::Engine`: the engine isolates
//!   transactions itself and says whether that is serializable.
//!
//! Index changes always go through the undo log, so indexes roll back with
//! the rows they point to, whatever the engine.
//!
//! Time is logical (RULES.md D29): each commit gets a commit timestamp from
//! one counter, so commit order is one number every engine agrees on, and
//! each transaction an id, unique, from another (`txn/clock.rs`). A
//! transaction reads
//! as of its snapshot: the highest timestamp below which every commit has
//! finished, so a commit that has taken its timestamp but not yet reached
//! every engine never shows in the middle of a reader's transaction. The
//! oldest snapshot of the open transactions is the horizon: versions no
//! snapshot at or after it can read are removed (`TableStorage::gc`)
//! when a transaction ends.
//!
//! The participants of a transaction are the tables whose engine it has
//! begun: on first write to an `Undo::Native` or `Concurrency::Engine`
//! engine, and on first read from a `Concurrency::Engine` one, which must
//! check at commit what was read. Commit is two-phase across them: the
//! transaction takes its commit timestamp, every participant must
//! `validate` at that timestamp before any is told to `commit` at it; if one
//! refuses, everything is rolled back. A transaction that wrote nothing
//! commits at its snapshot, with no timestamp and no validation: it read one
//! committed state, so it is serializable there.
//!
//! \[EXTENSION\] Below SERIALIZABLE [ISO/IEC 9075:1992 §4.28, §14.1 GR5],
//! a transaction reads in one of two ways (`Reading`): REPEATABLE READ at
//! its one snapshot, READ COMMITTED (and READ UNCOMMITTED) at a snapshot
//! taken as each statement begins, while the session has no cursor open.
//! Either way its reads are not checked at commit, but those made while
//! constraints are checked and referential actions run, which are
//! serializable at every level [§4.28]. The participants are told how it
//! reads (`TableStorage::read_mode`); an engine that is not, or that leaves
//! concurrency to the coordinator, reads as SERIALIZABLE.
//!
//! A conflicting lock request is refused or waits, as `LockWait` says, and a
//! wait that would deadlock is refused (`txn/locks.rs`, RULES.md D28).
//!
//! Nothing here is shared between transactions but the counter that numbers
//! them, the timestamps in use (commits under way and open snapshots, under
//! one short lock), each table's lock, and the graph of who waits for whom
//! (RULES.md D29): a transaction's undo log and bookkeeping are its own
//! (`Txn`), so transactions on different tables never wait for each other.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::mem;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::ThreadId;

use crate::catalog::QualifiedName;
use crate::db::State;
use crate::error::{Code, Result, SqlError};
use crate::log::{Log, Lsn};
use crate::sync::{Gate, lock, lock_builtin};

mod capabilities;
mod clock;
mod end;
mod locks;
mod logging;
mod reading;
mod stop;
mod undo;
mod write;

pub use capabilities::{Concurrency, Persistence, Policy, TxCapabilities, Undo};
use clock::Clock;
pub use locks::Damage;
pub(crate) use locks::TableLock;
use locks::WaitsFor;
pub use reading::{ReadMode, Reading};
pub use stop::Stop;
pub(crate) use stop::Window;
pub(crate) use stop::checked;
use undo::UndoLog;

/// How a transaction begins: whether it waits for locks, how it reads, and
/// what cancels its statements.
pub(crate) type Beginning = (LockWait, Reading, Arc<Stop>);
pub(crate) use undo::{Joined, UndoEntry, UndoSpace};

/// \[DESIGN\] A logical timestamp (RULES.md D29): a value of the counter
/// that orders the commits of a database's transactions, which snapshots
/// are read at. It never goes backwards and is never wall-clock time.
pub type Timestamp = u64;

/// A transaction's id: given in the order transactions begin, never reused
/// within a database, and ordering nothing against timestamps.
pub type TxId = u64;

/// \[DESIGN\] The transaction id that names no transaction: a read of the
/// latest committed state outside any transaction, as when an index is
/// built. No transaction is given this id.
pub const NO_TRANSACTION: TxId = 0;
/// The id of the first transaction the coordinator starts.
const FIRST_TRANSACTION: TxId = 1;

/// What happens to an open transaction when its session ends. The choice is
/// implementor-defined [X3.135-1986 §7.1 GR2].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEnd {
    /// Commit the open transaction.
    Commit,
    /// Roll the open transaction back.
    Rollback,
}

/// Whether a transaction that finds a table locked against it waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LockWait {
    /// The statement is refused at once (SQLCODE -911). Safe for a program
    /// that drives several sessions from one thread, where a wait could
    /// never end.
    #[default]
    NoWait,
    /// The statement waits until the holder ends its transaction; a wait
    /// that would deadlock is refused at once (SQLCODE -911). For sessions
    /// on threads of their own, as a server runs them.
    Wait,
}

/// The coordinator's table locks, weakest first: a held lock is only ever
/// strengthened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LockMode {
    /// Held by readers, and by writers of a table whose engine isolates
    /// transactions itself; any number of transactions may hold it together.
    Shared,
    /// \[DESIGN\] Held by a read that intends to write what it reads: by a
    /// searched UPDATE or DELETE on its table, and by an updatable cursor.
    /// One transaction may hold it, beside any number holding shared locks;
    /// so two such reads of a table take turns instead of deadlocking when
    /// each goes on to write.
    Update,
    /// Held by one writer, alone.
    Exclusive,
}

/// A transaction's own bookkeeping, which only its session touches.
#[derive(Default)]
pub(crate) struct TxState {
    undo: UndoLog,
    /// The tables whose engine has begun it, in the order they joined: the
    /// participants in commit and rollback, by the storage they joined, so
    /// a table dropped or given a new generation meanwhile ends there too.
    joined: Vec<Joined>,
    /// Where the current statement's undo entries begin.
    statement_start: usize,
    /// Savepoints taken in `Undo::Native` engines during the current statement.
    statement_savepoints: Vec<(Joined, u64)>,
    /// The table locks it holds, released when it ends.
    held: Vec<Arc<TableLock>>,
    /// Whether it wrote a change to the log, so its end must be logged too.
    logged: bool,
    /// The position of the current statement's first change in the log.
    statement_from: Option<Lsn>,
    /// Whether it has written a table: one that has not commits at its
    /// snapshot, without validation (`State::commit`).
    wrote: bool,
    /// Each table it wrote, with its lock, and the generation of the
    /// definitions it first wrote it under (`Txn::check_definitions`).
    written: Vec<(QualifiedName, Arc<TableLock>, u64)>,
    /// Why it can only roll back, if it can: a statement of it was undone
    /// but could not be logged as undone (`txn/logging.rs`), or could not
    /// be undone, and committing would commit what was left of it (RULES.md
    /// D19).
    doomed: Option<&'static str>,
    /// Whether the first table it wrote was of a `Persistence::External`
    /// engine, once it wrote one: a write of the other kind mixes them
    /// (`txn/logging.rs`).
    first_write_external: Option<bool>,
}

/// \[DESIGN\] What every snapshot of a database shares about transactions:
/// the clock that numbers them and gives their logical timestamps
/// (RULES.md D29, `txn/clock.rs`), how many are open, and who waits for
/// whom.
pub(crate) struct Transactions {
    clock: Clock,
    active: AtomicUsize,
    /// For each waiting transaction, the transactions it waits for.
    waits_for: Mutex<WaitsFor>,
    /// Taken by every change a checkpoint writes (`db/files.rs`).
    pub(crate) gate: Gate,
    /// The position of the first record of each open transaction that
    /// logged one: where recovery from a checkpoint begins reading.
    firsts: Mutex<BTreeMap<TxId, Lsn>>,
    /// Row changes logged since the last checkpoint.
    pub(crate) logged: AtomicU64,
    /// Index builds under way, which a checkpoint waits for: it records
    /// only indexes ready to be read (`Database::create_index`).
    builds: Mutex<usize>,
    /// Signalled when a build ends.
    built: Condvar,
    /// The failed sync of the log that stopped the database's changes, if
    /// one did (`txn/logging.rs`), and whether one did.
    stopped: Mutex<Option<SqlError>>,
    stopping: AtomicBool,
    /// The tables whose engines hold versions awaiting collection
    /// (`TableStorage::uncollected`), collected as any transaction ends.
    uncollected: Mutex<BTreeSet<QualifiedName>>,
    /// The tables whose engines have maintenance due (`TableStorage::maintenance_due`).
    maintenance: Mutex<BTreeSet<QualifiedName>>,
    /// What cancels the statements of each open transaction, for a plan's
    /// scans to look at (`txn/stop.rs`).
    stops: Mutex<HashMap<TxId, Arc<Stop>>>,
    /// The lock of each name a transaction defines, a table's or a view's,
    /// or grants or revokes privileges on (`db/definitions.rs`): the
    /// table's own once it is published, so the two are one.
    names: Mutex<HashMap<QualifiedName, std::sync::Weak<TableLock>>>,
}

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

    /// What cancels the statements of open transaction `tx`, if it is one.
    pub(crate) fn stop_of(&self, tx: TxId) -> Option<Arc<Stop>> {
        lock_builtin(&self.stops).get(&tx).cloned()
    }
}

impl Default for Transactions {
    fn default() -> Self {
        Transactions {
            clock: Clock::new(FIRST_TRANSACTION),
            active: AtomicUsize::new(0),
            waits_for: Mutex::new(HashMap::new()),
            gate: Gate::default(),
            firsts: Mutex::new(BTreeMap::new()),
            logged: AtomicU64::new(0),
            builds: Mutex::new(0),
            built: Condvar::new(),
            stopped: Mutex::new(None),
            stopping: AtomicBool::new(false),
            uncollected: Mutex::new(BTreeSet::new()),
            maintenance: Mutex::new(BTreeSet::new()),
            stops: Mutex::new(HashMap::new()),
            names: Mutex::new(HashMap::new()),
        }
    }
}

/// A commit under way, holding its timestamp: until it is dropped, no
/// snapshot counts commits at or after its timestamp as ended.
pub(crate) struct Committing<'a> {
    shared: &'a Transactions,
    /// Its commit timestamp.
    pub(crate) at: Timestamp,
}

impl Drop for Committing<'_> {
    fn drop(&mut self) {
        self.shared.clock.end_commit(self.at);
    }
}

impl Transactions {
    /// [X3.135-1986 §7.3 GR2a] a transaction is effectively initiated. Its id
    /// is new; 2^64 ids cannot run out in practice. Its snapshot is the
    /// stable timestamp then. Its changes and its end go to `log`, if there
    /// is one (RULES.md D25).
    /// Its undo entries spill in `space`.
    pub(crate) fn begin(self: &Arc<Self>, log: Option<Arc<dyn Log>>, (lock_wait, reading, stop): Beginning, space: UndoSpace) -> Txn {
        let (id, snapshot) = self.clock.begin();
        self.active.fetch_add(1, Ordering::SeqCst);
        lock_builtin(&self.stops).insert(id, Arc::clone(&stop));
        Txn {
            id,
            snapshot: AtomicU64::new(snapshot),
            reading,
            checking: AtomicBool::new(false),
            lock_wait,
            open: AtomicBool::new(true),
            shared: Arc::clone(self),
            log,
            state: Mutex::new(TxState { undo: UndoLog::new(space), ..TxState::default() }),
            running: Arc::new(Mutex::new(None)),
            stop,
        }
    }

    /// A commit timestamp, later than every one given out before, held
    /// until the commit ends.
    fn commit_timestamp(&self) -> Committing<'_> {
        Committing { shared: self, at: self.clock.take_commit() }
    }

    /// The oldest snapshot any open transaction reads at, or, with none
    /// open, the stable timestamp: no transaction reads before it.
    pub(crate) fn horizon(&self) -> Timestamp {
        self.clock.horizon()
    }

    /// Moves the clock past `highest`, a transaction id or timestamp found
    /// in the log, so no new transaction is given one again.
    pub(crate) fn advance_past(&self, highest: u64) {
        self.clock.advance_past(highest);
    }

    /// The highest transaction id or commit timestamp given, for a
    /// database reopened to go on past (`Database::close`).
    pub(crate) fn highest(&self) -> u64 {
        self.clock.highest()
    }

    /// The position of the first record of the oldest open transaction
    /// that logged one, if any.
    pub(crate) fn oldest_logged(&self) -> Option<Lsn> {
        lock_builtin(&self.firsts).values().min().copied()
    }

    /// Notes that `tx` logged its first record at `at`.
    pub(crate) fn first_logged(&self, tx: TxId, at: Lsn) {
        lock_builtin(&self.firsts).entry(tx).or_insert(at);
    }

    /// How many transactions are open.
    pub(crate) fn active(&self) -> usize {
        self.active.load(Ordering::SeqCst)
    }
}

/// \[DESIGN\] One transaction, owned by its session. Only that session uses
/// its bookkeeping, so its lock is never contended; other transactions see
/// only which thread its running call is on, to decide whether to wait for
/// it (`txn/locks.rs`).
pub(crate) struct Txn {
    /// Its id: unique, ordering nothing.
    pub(crate) id: TxId,
    /// The timestamp it reads at: what committed at or before it. Only
    /// its session moves it, as a statement begins.
    snapshot: AtomicU64,
    /// How it reads.
    reading: Reading,
    /// Whether its reads are checked though it reads below SERIALIZABLE:
    /// while constraints are checked.
    checking: AtomicBool,
    /// Whether it waits for a lock another holds: its session's setting
    /// when it began (`LOCK_WAIT`).
    pub(crate) lock_wait: LockWait,
    /// Whether its snapshot is still counted open.
    open: AtomicBool,
    shared: Arc<Transactions>,
    /// The log its changes and its end are written to, if any.
    log: Option<Arc<dyn Log>>,
    state: Mutex<TxState>,
    /// The thread its running call is on, if one is; shared with the locks it holds.
    running: Arc<Mutex<Option<ThreadId>>>,
    /// What cancels its statements: its session's (`txn/stop.rs`).
    pub(crate) stop: Arc<Stop>,
}

impl Txn {
    /// Its bookkeeping.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a thread panicked while holding it.
    fn state(&self) -> Result<MutexGuard<'_, TxState>> {
        lock(&self.state)
    }

    /// Records `entry`, which undoes a change it made.
    fn log(&self, entry: UndoEntry) -> Result<()> {
        self.state()?.undo.push(entry)
    }

    /// Marks where the statement now starting begins, so that it alone can be
    /// undone if it fails [X3.135-1986 §7.3 GR3b i]. Reading at a snapshot
    /// for each statement, it takes a new one if `fresh` says so: when its
    /// session has no cursor open, which reads at the one it has.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if its lock is poisoned; a participant's, told of the
    /// new snapshot.
    pub(crate) fn begin_statement(&self, fresh: impl FnOnce() -> bool) -> Result<()> {
        self.check_definitions()?;
        {
            let mut st = self.state()?;
            st.statement_start = st.undo.mark();
            st.statement_savepoints.clear();
            st.statement_from = None;
        }
        if self.reading == Reading::Statement && self.open.load(Ordering::Acquire) && fresh() {
            self.snapshot.store(self.shared.clock.moved(self.id, self.snapshot()), Ordering::SeqCst);
            self.tell_participants()?;
        }
        Ok(())
    }

    /// Records the thread its running call is on, or that none runs.
    pub(crate) fn runs_on(&self, thread: Option<ThreadId>) {
        *lock_builtin(&self.running) = thread;
    }

    /// Releases every table lock it holds and wakes the transactions waiting
    /// for them, and forgets its snapshot. Called when it ends, and again
    /// when it is dropped, which finds nothing left to release.
    fn release(&self) {
        let held = mem::take(&mut lock_builtin(&self.state).held);
        for table in held {
            table.release(self.id);
        }
        if self.open.swap(false, Ordering::AcqRel) {
            self.shared.clock.end(self.id, self.snapshot());
        }
    }
}

impl Drop for Txn {
    fn drop(&mut self) {
        self.release();
        lock_builtin(&self.shared.firsts).remove(&self.id);
        lock_builtin(&self.shared.stops).remove(&self.id);
        self.shared.active.fetch_sub(1, Ordering::SeqCst);
    }
}

/// An undo step that failed, and the table it leaves damaged, if one is known.
struct UndoFailure {
    table: Option<QualifiedName>,
    error: SqlError,
    /// Whether it was a row's change that failed, not an index entry's.
    rows: bool,
}

// ---------------------------------------------------------------------------
// Transactional operations on the database state. Every change to stored
// rows goes through here, so locking, undo and index maintenance happen in
// one place.
// ---------------------------------------------------------------------------

impl State {
    pub(crate) fn capabilities(&self, table: &QualifiedName) -> Result<TxCapabilities> {
        Ok(self.storage_of(table)?.capabilities())
    }

    fn check_isolation(&self, table: &QualifiedName, caps: TxCapabilities) -> Result<()> {
        if caps.concurrency == (Concurrency::Engine { serializable: false }) && self.policy() == Policy::Strict {
            return Err(SqlError::new(
                Code::Policy,
                format!(
                    "[X3.135-1986 §4.16] requires serializable execution; the engine of {table} does not provide \
                     it, and the transaction policy is Strict"
                ),
            ));
        }
        Ok(())
    }

    /// The damaged tables, sorted by name.
    pub fn damaged_tables(&self) -> Vec<QualifiedName> {
        // A table dropped is no longer damaged: its lock may stay.
        let mut tables: Vec<QualifiedName> =
            self.locks.iter().filter(|(t, lk)| lk.is_damaged() && self.catalog.table(t).is_some()).map(|(t, _)| t.clone()).collect();
        tables.sort_by_key(ToString::to_string);
        tables
    }

    /// Refuses any use of a damaged table (RULES.md D19).
    fn check_damaged(&self, table: &QualifiedName) -> Result<()> {
        if self.lock_of(table)?.is_damaged() {
            return Err(SqlError::new(Code::Damaged, format!("[DESIGN] {table} is damaged: a change to it could not be undone")));
        }
        Ok(())
    }

    /// Before reading a table: a shared lock, so that the execution is
    /// serializable [X3.135-1986 §4.16].
    pub(crate) fn read_lock(&self, tx: &Txn, table: &QualifiedName) -> Result<()> {
        self.lock_to_read(tx, table, LockMode::Shared)
    }

    /// Before reading a table to write the rows read: an update lock
    /// (`LockMode::Update`).
    pub(crate) fn update_lock(&self, tx: &Txn, table: &QualifiedName) -> Result<()> {
        self.lock_to_read(tx, table, LockMode::Update)
    }

    /// Before reading a table, a lock in `mode` if its engine leaves
    /// concurrency to the coordinator.
    fn lock_to_read(&self, tx: &Txn, table: &QualifiedName, mode: LockMode) -> Result<()> {
        self.check_damaged(table)?;
        self.lock_of(table)?.check_generation(table, self.generation_of(table), tx)?;
        let caps = self.capabilities(table)?;
        self.check_isolation(table, caps)?;
        match caps.concurrency {
            Concurrency::Coordinator => self.acquire(tx, table, mode),
            Concurrency::Engine { .. } => self.join(tx, table),
        }
    }

    /// Makes `table` a participant of `tx`, beginning the transaction in its
    /// engine, unless it is one already. A table joins only once its engine
    /// has begun the transaction, so commit and rollback never reach an
    /// engine whose begin failed. The transaction's bookkeeping
    /// is not locked across the engine's `begin` (RULES.md R-ERR-4).
    fn join(&self, tx: &Txn, table: &QualifiedName) -> Result<()> {
        let joined = Joined { table: table.clone(), storage: Arc::clone(self.storage_of(table)?) };
        if tx.state()?.joined.iter().any(|j| j.is(&joined.storage)) {
            return Ok(());
        }
        joined.storage.begin(tx.id, tx.snapshot())?;
        if tx.reading != Reading::Serializable {
            joined.storage.read_mode(tx.id, tx.read_mode())?;
        }
        tx.state()?.joined.push(joined);
        Ok(())
    }

    /// Before changing a table: policy, lock, participant and savepoint bookkeeping.
    fn prepare_write(&self, tx: &Txn, table: &QualifiedName) -> Result<()> {
        self.check_damaged(table)?;
        self.lock_of(table)?.check_generation(table, self.generation_of(table), tx)?;
        let caps = self.capabilities(table)?;
        self.check_isolation(table, caps)?;
        if caps.undo == Undo::Unsupported && self.policy() == Policy::Strict {
            return Err(SqlError::new(
                Code::Policy,
                format!(
                    "[X3.135-1986 §4.16] requires changes to be cancellable; the engine of {table} cannot cancel \
                     them, and the transaction policy is Strict"
                ),
            ));
        }
        self.check_persistence(tx, table, caps.persistence)?;
        tx.state()?.wrote = true;
        let storage = self.storage_of(table)?;
        // An engine that isolates transactions itself lets its writers share
        // the table's lock with each other, but not with `create_index`, which
        // must see no uncommitted change (`Database::create_index`).
        let mode = if caps.concurrency == Concurrency::Coordinator { LockMode::Exclusive } else { LockMode::Shared };
        self.acquire(tx, table, mode)?;
        // Holding the lock, the statement may write only if its snapshot
        // knows every index the table has (RULES.md D27).
        if self.lock_of(table)?.changed_after(self.generation) {
            return Err(SqlError::new(
                Code::Conflict,
                format!("[DESIGN] the definitions of {table} changed after the statement began; run it again"),
            ));
        }
        // An index published from here on dooms the transaction instead.
        tx.wrote_under(table, self.lock_of(table)?, self.generation)?;
        if caps.undo == Undo::Native || caps.concurrency != Concurrency::Coordinator {
            self.join(tx, table)?;
        }
        // Only the transaction's own session touches its bookkeeping, so it
        // is the same before and after the engine's `savepoint`.
        let needs_savepoint = caps.undo == Undo::Native && !tx.state()?.statement_savepoints.iter().any(|(j, _)| j.is(storage));
        if needs_savepoint {
            let sp = storage.savepoint(tx.id);
            tx.state()?.statement_savepoints.push((Joined { table: table.clone(), storage: Arc::clone(storage) }, sp));
        }
        Ok(())
    }
}

// Unit tests: commit reports a participant it cannot find (RULES.md R-ERR-1,
// R-TEST-2 robustness).
#[cfg(test)]
mod tests;
