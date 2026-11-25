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
//! finished and is durable, so a commit that has taken its timestamp but
//! not yet reached every engine never shows in the middle of a reader's
//! transaction. The oldest snapshot of the open transactions is the
//! horizon: versions no snapshot at or after it can read are removed
//! (`TableStorage::gc`) when a transaction ends.
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
//! Nothing here is shared between transactions but the clock that numbers
//! them and keeps the timestamps in use (commits under way and open
//! snapshots, taken without a global lock: `txn/clock.rs`), each table's
//! lock, the graph of who waits for whom, and a few maps held only for an
//! insert or a removal as a transaction begins, logs or ends (RULES.md
//! D29): a transaction's undo log and bookkeeping are its own (`Txn`), so
//! transactions on different tables take no lock in common. A commit still
//! waits for every commit before it to end, whatever tables they touch.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::thread::ThreadId;

use crate::catalog::QualifiedName;
use crate::error::{Result, SqlError};
use crate::sync::{Alone, Gate, lock, lock_builtin};

mod access;
mod begin;
mod capabilities;
mod clock;
mod collect;
mod durable;
mod end;
pub(crate) mod escrow;
mod followed;
mod group;
mod kept;
mod lobs;
mod locks;
mod logging;
mod marks;
mod owed;
mod pending;
mod reading;
mod release;
mod row_waits;
mod savepoints;
mod stop;
mod undo;
mod write;
pub(crate) mod writing;

pub(crate) use begin::Origin;
pub use capabilities::{Concurrency, Persistence, Policy, TxCapabilities, Undo};
use clock::Clock;
pub(crate) use collect::remove_entries;
pub(crate) use durable::Committing;
pub(crate) use followed::{Followed, FollowedRecords, Following};
pub(crate) use kept::Sharing;
pub use locks::Damage;
pub(crate) use locks::TableLock;
use locks::WaitsFor;
pub(crate) use logging::Held;
pub(crate) use marks::Mark;
use pending::{First, LogPos, Pending};
pub use reading::{ReadMode, Reading};
use stop::Stops;
pub(crate) use stop::Window;
pub use stop::{Activity, MAX_NOTES, Now, Served, Stop, Tallied, Tallies, Tally, Waiting};
pub(crate) use stop::{checked, counted, debuggers_attached};
use undo::UndoLog;

/// How a transaction begins: whether it waits for locks, how it reads, and
/// what cancels its statements.
pub(crate) type Beginning = (LockWait, Reading, Arc<Stop>);
pub(crate) use undo::{Joined, UndoEntry, UndoSpace};
pub(crate) use write::Again;

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
    /// The escrows of the tables it reserved increments in (RULES.md D452).
    escrowed: Vec<Arc<escrow::Escrow>>,
    /// Whether it wrote a change to the log, so its end must be logged too.
    logged: bool,
    /// The snapshots it read at before its current one that it keeps
    /// counted open until it ends, as a locator holds a value read at one
    /// (`Txn::pin_snapshot`, RULES.md D79).
    pinned: Vec<Timestamp>,
    /// The place of the current statement's first change in the log.
    statement_from: Option<LogPos>,
    /// Of each mark open within the current statement, innermost last, the
    /// place of its first change in the log (`marks.rs`).
    marks_from: Vec<Option<LogPos>>,
    /// Its savepoints (`txn/savepoints.rs`).
    savepoints: savepoints::Savepoints,
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
    /// Whether it wrote a table of an engine that makes its own writes
    /// durable (`Persistence::Forced`): its records are then written as
    /// made, before the engine's writes can be (`txn/pending.rs`).
    forced: bool,
    /// The rows whose entry in a unique index, named, was the key's only
    /// one as it was last inserted (`Index::insert_alone`): no other row
    /// to look for at a statement's end (`db/assignment.rs`); by index, so
    /// a row is looked up by the index's name as it is held.
    alone: HashMap<String, HashSet<crate::storage::RowId>>,
    /// Of each table of an assertion's kept totals and form, the row it
    /// wrote of its change, each statement's added to it
    /// (`db/assertions/kept.rs`).
    kept_rows: HashMap<(QualifiedName, usize), crate::storage::RowId>,
}

/// \[DESIGN\] What every snapshot of a database shares about transactions:
/// the clock that numbers them and gives their logical timestamps
/// (RULES.md D29, `txn/clock.rs`), how many are open, and who waits for
/// whom.
pub(crate) struct Transactions {
    /// The clock, which counts the transactions open too.
    clock: Clock,
    /// For each waiting transaction, the transactions it waits for.
    waits_for: Mutex<WaitsFor>,
    /// Taken by every change a checkpoint writes (`db/checkpoint.rs`).
    pub(crate) gate: Gate,
    /// The first record of each open transaction that logged one: its
    /// position, where recovery from a checkpoint begins reading.
    firsts: Mutex<BTreeMap<TxId, First>>,
    /// Row changes logged since the database was opened.
    pub(crate) logged: Alone<AtomicU64>,
    /// The records open transactions hold (`txn/pending.rs`).
    pending: Pending,
    /// Of those, the ones the last checkpoint taken holds: those after it
    /// count toward the next (CHECKPOINT.AFTER_CHANGES).
    checkpointed: AtomicU64,
    /// Index builds under way, which a checkpoint waits for: it records
    /// only indexes ready to be read (`Database::create_index`).
    builds: Mutex<usize>,
    /// Signalled when a build ends.
    built: Condvar,
    /// The failed sync of the log that stopped the database's changes, if
    /// one did (`txn/logging.rs`), and whether one did.
    stopped: Mutex<Option<SqlError>>,
    stopping: Alone<AtomicBool>,
    /// The commits written to the log but not yet synced, under
    /// LOG.COMMIT_SYNC OFF, if any (`txn/logging.rs`).
    unsynced: Mutex<Option<logging::Unsynced>>,
    /// What holds the database's changes beside a failed sync, if anything
    /// does (`txn/logging.rs`, RULES.md D57), and whether something does.
    held: Mutex<Option<Held>>,
    holding: Alone<AtomicBool>,
    /// Following a log, the records of each of its transactions not yet
    /// ended, held until it ends (`db/standby.rs`), spilled past a work
    /// area's bytes (`txn/followed.rs`).
    pub(crate) followed: Mutex<BTreeMap<TxId, FollowedRecords>>,
    /// Following a log, where it stands in it (`db/standby.rs`).
    pub(crate) following: Mutex<Following>,
    /// Held while a following database replays, or takes a checkpoint, so
    /// the two take turns.
    pub(crate) replaying: crate::threads::Turn,
    /// The tables whose engines hold versions awaiting collection
    /// (`TableStorage::uncollected`), collected by the end of a
    /// transaction that wrote them once a batch is long due, or by the
    /// maintenance job's pass (RULES.md D451, D920).
    uncollected: Mutex<BTreeSet<QualifiedName>>,
    /// Whether `uncollected` names any table, as it was last changed: so
    /// a transaction's end looks at it without its lock.
    awaiting: Alone<AtomicBool>,
    /// Whether the pass collecting `uncollected` has been asked for and
    /// has not begun: an end that leaves a table awaiting asks once
    /// (RULES.md D920).
    pass_asked: AtomicBool,
    /// When it was asked for, while it is: none if it is to be made at
    /// once (RULES.md D925).
    pass_since: Mutex<Option<std::time::Instant>>,
    /// Whether it was asked for at once and has not begun, and whether
    /// its job was asked for since.
    pass_now: AtomicBool,
    pass_told: AtomicBool,
    /// The tables whose engines have maintenance due (`TableStorage::maintenance_due`).
    maintenance: Mutex<BTreeSet<QualifiedName>>,
    /// Whether `maintenance` names any table.
    noted: AtomicBool,
    /// The tables of assertions' kept totals with rows to fold
    /// (`db/assertions/fold.rs`).
    folds: Mutex<BTreeSet<QualifiedName>>,
    /// What cancels the statements of each open transaction, for a plan's
    /// scans to look at (`txn/stop.rs`).
    stops: Stops,
    /// The end of each open transaction, which waiters for its rows queue
    /// on (`txn/row_waits.rs`).
    ends: row_waits::Ends,
    /// The lock of each name a transaction defines, a table's or a view's,
    /// or grants or revokes privileges on (`db/definitions.rs`): the
    /// table's own once it is published, so the two are one.
    names: Mutex<HashMap<QualifiedName, std::sync::Weak<TableLock>>>,
    /// The commits waiting for a sync of the log (`txn/group.rs`).
    group: group::Group,
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
        self.stops.of(tx)
    }
}

impl Default for Transactions {
    fn default() -> Self {
        Transactions {
            clock: Clock::new(FIRST_TRANSACTION),
            waits_for: Mutex::new(HashMap::new()),
            gate: Gate::default(),
            firsts: Mutex::new(BTreeMap::new()),
            logged: Alone(AtomicU64::new(0)),
            pending: Pending::default(),
            checkpointed: AtomicU64::new(0),
            builds: Mutex::new(0),
            built: Condvar::new(),
            stopped: Mutex::new(None),
            stopping: Alone(AtomicBool::new(false)),
            unsynced: Mutex::new(None),
            held: Mutex::new(None),
            holding: Alone(AtomicBool::new(false)),
            followed: Mutex::new(BTreeMap::new()),
            following: Mutex::new(Following::default()),
            replaying: crate::threads::Turn::default(),
            uncollected: Mutex::new(BTreeSet::new()),
            awaiting: Alone(AtomicBool::new(false)),
            pass_asked: AtomicBool::new(false),
            pass_since: Mutex::new(None),
            pass_now: AtomicBool::new(false),
            pass_told: AtomicBool::new(false),
            maintenance: Mutex::new(BTreeSet::new()),
            noted: AtomicBool::new(false),
            folds: Mutex::new(BTreeSet::new()),
            stops: Stops::default(),
            ends: row_waits::Ends::default(),
            names: Mutex::new(HashMap::new()),
            group: group::Group::default(),
        }
    }
}

impl Transactions {
    /// The oldest snapshot any open transaction reads at, or, with none
    /// open, the durable timestamp: no transaction reads before it.
    pub(crate) fn horizon(&self) -> Timestamp {
        self.clock.horizon()
    }

    /// Moves the clock past `highest`, a transaction id or timestamp found
    /// in the log, so no new transaction is given one again.
    pub(crate) fn advance_past(&self, highest: u64) {
        self.clock.advance_past(highest);
    }

    /// The highest transaction id or commit timestamp given, kept in each
    /// checkpoint for a database reopened to go on past (`db/checkpoint.rs`).
    pub(crate) fn highest(&self) -> u64 {
        self.clock.highest()
    }

    /// The latest commit timestamp given: a timestamp, never an id, which
    /// are given 64 apart and so run ahead of commits (RULES.md D844).
    pub(crate) fn latest_commit(&self) -> Timestamp {
        self.clock.latest_commit()
    }

    /// Row changes logged since the last checkpoint taken.
    pub(crate) fn since_checkpoint(&self) -> u64 {
        self.logged.load(Ordering::Relaxed).saturating_sub(self.checkpointed.load(Ordering::Relaxed))
    }

    /// Notes a checkpoint taken, holding the first `logged` row changes.
    pub(crate) fn checkpointed(&self, logged: u64) {
        self.checkpointed.fetch_max(logged, Ordering::Relaxed);
    }

    /// How many transactions are open.
    pub(crate) fn active(&self) -> usize {
        self.clock.active()
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
    /// when it began (`LOCKS.WAIT`).
    pub(crate) lock_wait: LockWait,
    /// Whether its commit waits for its commit record to be synced: its
    /// session's LOG.COMMIT_SYNC as it commits, set then
    /// (`Txn::set_sync_commit`); so by default.
    sync_commit: AtomicBool,
    /// Whether its snapshot is still counted open.
    open: AtomicBool,
    /// What it began from: the coordinator, and the log its changes and
    /// its end are written to, if any (`txn/begin.rs`).
    origin: Arc<Origin>,
    state: Mutex<TxState>,
    /// The thread its running call is on, if one is; shared with the locks it holds.
    running: Arc<Mutex<Option<ThreadId>>>,
    /// What cancels its statements: its session's (`txn/stop.rs`).
    pub(crate) stop: Arc<Stop>,
    /// [ISO/IEC 9075-2:2023 §4.44] Its transaction timestamp, in
    /// microseconds of UTC, once its first change set it (`Txn::stamp`).
    stamp: OnceLock<i64>,
    /// The records it holds, once it holds one (`txn/pending.rs`).
    pending: OnceLock<Arc<pending::TxLog>>,
    /// The latest commit not yet durable it read past its snapshot
    /// (`txn/durable.rs`), 0 for none.
    depends: AtomicU64,
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

    /// [ISO/IEC 9075-2:2023 §4.44] Its transaction timestamp, in
    /// microseconds of UTC: set, by `now`, before its first change, and
    /// the same for the rest of it.
    pub(crate) fn stamp(&self, now: impl FnOnce() -> i64) -> i64 {
        *self.stamp.get_or_init(now)
    }

    /// Records `entry`, which undoes a change it made.
    pub(crate) fn log(&self, entry: UndoEntry) -> Result<()> {
        self.state()?.undo.push(entry)
    }

    /// Whether it has written a table or definitions, so its end may
    /// leave a checkpoint, a fold of kept totals or a sync of the log due.
    pub(crate) fn wrote(&self) -> bool {
        lock_builtin(&self.state).wrote
    }

    /// Marks where the statement now starting begins, so that it alone can be
    /// undone if it fails [X3.135-1986 §7.3 GR3b i]. Reading at a snapshot
    /// for each statement, it takes a new one if `fresh` says so: when its
    /// session has no cursor open, which reads at the one it has.
    ///
    /// Whether a load is under way in it (RULES.md D532), which a statement
    /// that is no part of it ends first.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if its lock is poisoned; a participant's, told of the
    /// new snapshot.
    pub(crate) fn begin_statement(&self, fresh: impl FnOnce() -> bool) -> Result<bool> {
        self.check_definitions()?;
        let loads = {
            let mut st = self.state()?;
            st.statement_start = st.undo.mark();
            st.statement_savepoints.clear();
            st.statement_from = None;
            st.clear_marks();
            self.settle_places(&mut st);
            st.undo.loads()
        };
        self.renew_snapshot(fresh)?;
        Ok(loads)
    }

    /// Reading at a snapshot for each statement, takes a new one if
    /// `fresh` says so: as a statement begins, and as each statement of a
    /// procedure's body does (RULES.md D454).
    ///
    /// # Errors
    ///
    /// A participant's, told of the new snapshot.
    pub(crate) fn renew_snapshot(&self, fresh: impl FnOnce() -> bool) -> Result<()> {
        if self.reading == Reading::Statement && self.open.load(Ordering::Acquire) && fresh() {
            self.snapshot.store(self.origin.shared.clock.moved(self.id, self.snapshot()), Ordering::SeqCst);
            self.tell_participants()?;
        }
        Ok(())
    }

    /// Records the thread its running call is on, or that none runs.
    pub(crate) fn runs_on(&self, thread: Option<ThreadId>) {
        *lock_builtin(&self.running) = thread;
    }
}

impl Drop for Txn {
    fn drop(&mut self) {
        self.release();
        // Only one whose records were written before its end has a first
        // to forget.
        if self.pinned_log() {
            lock_builtin(&self.origin.shared.firsts).remove(&self.id);
        }
        self.forget_held();
        self.origin.shared.stops.remove(self.id);
        self.origin.shared.clock.dropped(self.id);
    }
}

/// An undo step that failed, and the table it leaves damaged, if one is known.
struct UndoFailure {
    table: Option<QualifiedName>,
    error: SqlError,
    /// Whether it was a row's change that failed, not an index entry's.
    rows: bool,
}

#[cfg(test)]
mod tests;
