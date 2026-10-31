//! \[DESIGN\] What transactions write to the database's log (RULES.md D25):
//! each row change as it is made, with its images; a statement's changes,
//! when it fails, marked undone; and the transaction's end. A transaction
//! commits when its commit record is durable: `sync` of it returns.
//! Definitions changed outside the transactions of sessions (settings,
//! users, schemas, indexes) are logged and synced before they are
//! published. Split from txn.rs (RULES.md R-SIZE-4).
//!
//! \[DESIGN\] A failed sync of the log stops the database's changes (RULES.md
//! D25): what the log held may be lost, and a later sync may succeed
//! without it, as some systems forget a failed write once they report it;
//! so from the first, nothing that writes the log is accepted: a row change,
//! the commit of a transaction that wrote, a definition, a checkpoint,
//! whose image would claim what is not durable. Reads, rollbacks and the
//! commits of transactions that wrote nothing go on. The transaction whose
//! commit record's sync failed is in doubt: never made visible, its locks
//! and versions held, its session told (SQLCODE -922). Opening the
//! database again recovers it from what its log holds, committing it if
//! its commit record is there.
//!
//! \[DESIGN\] A commit that does not wait for the log (LOG.COMMIT_SYNC OFF)
//! is noted unsynced, with its log and position, until a sync of that log
//! through it: the log's sync job (`db/log_sync.rs`) syncs it within
//! LOG.SYNC_AFTER_MS, unless another sync does first (RULES.md D25).
//!
//! The in-memory undo log stays: it undoes statements and transactions, and
//! the index entries the log does not hold. The log's before-images are for
//! recovery's undo (RULES.md D25).

use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::{NO_TRANSACTION, Persistence, Policy, Timestamp, Transactions, Txn};
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::disk::encoding::Pipeline;
use crate::error::{Code, Result, SqlError, SqlState};
use crate::log::record::{Change, Record};
use crate::log::sealed;
use crate::log::{Log, Lsn};
use crate::storage::RowId;
use crate::sync::lock_builtin;

/// \[DESIGN\] What holds a database's changes as it stands in a group of
/// replicas (RULES.md D57, `db/standby.rs`): a row change, the commit of a
/// transaction that wrote, a definition or a checkpoint is refused, as a
/// failed sync refuses them; reads and rollbacks go on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Held {
    /// It follows a log another replica writes, until it is promoted.
    Following,
    /// It served as primary and was deposed: until it is opened again.
    Deposed,
}

impl Held {
    /// The error a change is refused with: SQLCODE -917, as a change is in
    /// a READ ONLY transaction [ISO/IEC 9075:1992 §13.6-§13.10 GR1].
    fn refusal(self) -> SqlError {
        let why = match self {
            Held::Following => "the database follows a log another replica writes, so it takes changes only once it is promoted",
            Held::Deposed => "the database was deposed as primary, so it takes no change until it is opened again",
        };
        SqlError::new(Code::TransactionState, format!("[DESIGN] RULES.md D57: {why}"))
    }
}

impl Transactions {
    /// Whether the database takes changes: not once a sync of its log
    /// failed, nor while it is held (`Held`).
    ///
    /// # Errors
    ///
    /// SQLCODE -915 naming the failure, once one did; -917 while it is
    /// held.
    pub(crate) fn taking_changes(&self) -> Result<()> {
        if self.stopping.load(Ordering::Acquire) {
            let cause = lock_builtin(&self.stopped).as_ref().map_or_else(String::new, SqlError::described);
            return Err(SqlError::new(
                Code::Io,
                format!(
                    "[DESIGN] a sync of the log failed, so the database takes no change until it is opened again and recovered: {cause}"
                ),
            ));
        }
        if !self.holding.load(Ordering::Acquire) {
            return Ok(());
        }
        lock_builtin(&self.held).map_or(Ok(()), |held| Err(held.refusal()))
    }

    /// What holds the database's changes beside a failed sync, if anything
    /// does (RULES.md D57).
    pub(crate) fn held(&self) -> Option<Held> {
        *lock_builtin(&self.held)
    }

    /// Holds the database's changes for `held`, or, for none, takes them
    /// again.
    pub(crate) fn hold(&self, held: Option<Held>) {
        let mut slot = lock_builtin(&self.held);
        *slot = held;
        self.holding.store(held.is_some(), Ordering::Release);
    }

    /// Syncs `log` through `lsn`, once the database takes changes; a
    /// failure stops it taking them.
    ///
    /// # Errors
    ///
    /// As `taking_changes`; the log's.
    pub(crate) fn sync(&self, log: &dyn Log, lsn: Lsn) -> Result<()> {
        self.taking_changes()?;
        log.sync(lsn).inspect_err(|e| {
            lock_builtin(&self.stopped).get_or_insert_with(|| e.clone());
            self.stopping.store(true, Ordering::Release);
        })?;
        self.synced(log, lsn);
        Ok(())
    }

    /// Notes the commit record at `lsn` of `log` written, not synced: the
    /// unsynced commits noted are then through it. A commit record follows
    /// a change of its transaction, so its position is never 0. A log set
    /// in place of the one noted (`Database::set_log`) takes its place:
    /// the other is synced only as its host syncs it.
    fn note_unsynced(&self, log: &Arc<dyn Log>, lsn: Lsn) {
        let mut unsynced = lock_builtin(&self.unsynced);
        if let Some(noted) = unsynced.as_mut().filter(|u| same_log(&u.log, &**log)) {
            noted.through = noted.through.max(lsn);
        } else {
            *unsynced = Some(Unsynced { log: Arc::clone(log), through: lsn, asked: false });
        }
    }

    /// Forgets the unsynced commits a sync of `log` through `lsn` made
    /// durable: all, or none.
    fn synced(&self, log: &dyn Log, lsn: Lsn) {
        let mut unsynced = lock_builtin(&self.unsynced);
        if unsynced.as_ref().is_some_and(|u| same_log(&u.log, log) && u.through <= lsn) {
            *unsynced = None;
        }
    }

    /// Whether the log's sync job is to be asked for: commits wait
    /// unsynced, and it was not asked for them yet; it is then noted asked.
    pub(crate) fn ask_sync(&self) -> bool {
        lock_builtin(&self.unsynced).as_mut().is_some_and(|u| !std::mem::replace(&mut u.asked, true))
    }

    /// The log of the unsynced commits, and the last one's position, no
    /// longer noted: to be synced. A commit written after is noted anew.
    pub(crate) fn take_unsynced(&self) -> Option<(Arc<dyn Log>, Lsn)> {
        lock_builtin(&self.unsynced).take().map(|u| (u.log, u.through))
    }

    /// The highest timestamp below which every commit has ended.
    pub(crate) fn stable(&self) -> Timestamp {
        self.clock.stable()
    }

    /// The failure that stopped the database's changes, if one did.
    pub(crate) fn stopped_by(&self) -> Option<SqlError> {
        lock_builtin(&self.stopped).clone()
    }
}

/// \[DESIGN\] Commits written to a log and not yet synced: the log, the
/// last one's position, and whether the log's sync job was asked for them.
pub(crate) struct Unsynced {
    log: Arc<dyn Log>,
    through: Lsn,
    asked: bool,
}

/// Whether `a` and `b` are one log.
fn same_log(a: &Arc<dyn Log>, b: &dyn Log) -> bool {
    std::ptr::addr_eq(Arc::as_ptr(a), std::ptr::from_ref(b))
}

impl Txn {
    /// Writes `change`, made by this transaction, to its log, if it has one.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 once a sync of the log failed; the log's, if it cannot
    /// append the record; the statement then fails, and is undone.
    pub(crate) fn log_change(&self, change: Change) -> Result<()> {
        self.log_sealed(change, None)
    }

    /// As `log_change`, the record sealed through `seal`'s pipeline, of its
    /// row, if given (`log/sealed.rs`): a row change of a table with
    /// encodings.
    pub(crate) fn log_sealed(&self, change: Change, seal: Option<(&Pipeline, RowId)>) -> Result<()> {
        let Some(log) = &self.log else { return Ok(()) };
        self.shared.taking_changes()?;
        let record = Record { tx: self.id, change };
        let bytes = if let Some((pipeline, row)) = seal { sealed::seal(&record, row, pipeline)? } else { record.encode() };
        let lsn = log.append(&bytes)?;
        let mut st = self.state()?;
        if !st.logged {
            self.shared.first_logged(self.id, lsn);
        }
        st.logged = true;
        st.statement_from.get_or_insert(lsn);
        self.shared.logged.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Marks the current statement's changes undone in the log, if it wrote
    /// any. If that cannot be written, the log would show changes that were
    /// cancelled as made, so the transaction can then only roll back.
    pub(super) fn statement_undone(&self) {
        let from = lock_builtin(&self.state).statement_from.take();
        let (Some(log), Some(from)) = (&self.log, from) else { return };
        let record = Record { tx: self.id, change: Change::StatementUndone { from } }.encode();
        // Appended with the state unlocked: the log is a plug-in (RULES.md
        // R-ERR-4).
        if log.append(&record).is_err() {
            lock_builtin(&self.state).doomed.get_or_insert("a statement's changes were undone but could not be logged as undone");
        }
    }

    /// Whether its commit waits for its commit record to be synced: as its
    /// session's LOG.COMMIT_SYNC says as it commits.
    pub(crate) fn set_sync_commit(&self, sync: bool) {
        self.sync_commit.store(sync, Ordering::Relaxed);
    }

    /// Writes the commit record, at commit timestamp `at`, and syncs it, if
    /// the transaction logged a change, unless its session's LOG.COMMIT_SYNC
    /// is OFF (`set_sync_commit`): then the record is noted unsynced, and
    /// synced by the log's next sync, of a later commit, a definition, a
    /// checkpoint or the log's sync job, each of which syncs every record
    /// before its own; until then a failure of
    /// the system loses it, and recovery undoes the transaction, as every
    /// one committed after it whose record is lost with it. The commit
    /// record's position is the order of commits (RULES.md D25).
    ///
    /// # Errors
    ///
    /// SQLCODE -913 if the transaction must roll back instead: the commit
    /// record could not be written. Inside `Ok`, the outcome of the sync:
    /// SQLCODE -922 if it failed, the transaction then in doubt (RULES.md
    /// D25).
    pub(super) fn commit_record(&self, at: Timestamp) -> Result<Result<()>> {
        let logged = self.state()?.logged;
        let Some(log) = self.log.as_ref().filter(|_| logged) else { return Ok(Ok(())) };
        let refused =
            |why: String| SqlError::new(Code::CommitFailed, format!("[X3.135-1986 §8.2] {why}, so the transaction was rolled back"));
        self.shared.taking_changes().map_err(|e| refused(e.described()))?;
        let record = Record { tx: self.id, change: Change::Commit { at } }.encode();
        let lsn = log.append(&record).map_err(|e| refused(format!("the commit record could not be written: {}", e.described())))?;
        if !self.sync_commit.load(Ordering::Relaxed) {
            self.shared.note_unsynced(log, lsn);
            return Ok(Ok(()));
        }
        Ok(self.shared.sync(&**log, lsn).map_err(|e| {
            SqlError::new(
                Code::InDoubt,
                format!(
                    "[DESIGN] the commit record could not be synced, so whether the transaction committed is known only once the database is opened again and recovered from its log; until then no other session sees its changes: {}",
                    e.described()
                ),
            )
            .with_state(SqlState::TRANSACTION_RESOLUTION_UNKNOWN)
        }))
    }

    /// Writes the abort record, if the transaction logged a change. Not
    /// synced: a transaction without a commit record is undone by recovery
    /// either way, and a failure to write it cannot matter for the same
    /// reason. Not written while the database is held (`Held`): a deposed
    /// database's log is its replica's, which may serve again, in a later
    /// view, for the database opened again (RULES.md D57).
    pub(super) fn log_abort(&self) {
        if self.shared.held().is_some() {
            return;
        }
        if let Some(log) = self.log.as_ref().filter(|_| lock_builtin(&self.state).logged) {
            let _ = log.append(&Record { tx: self.id, change: Change::Abort }.encode());
        }
    }
}

impl State {
    /// Records `change`, a change of definitions made outside any
    /// transaction, or by `tx`, a transaction of its own (an index's,
    /// `db/indexes.rs`), among the definitions, and writes it to the log,
    /// and syncs it: called before the change is published, so a
    /// definition the log does not hold never is.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 once a sync of the log failed; the log's, if it cannot
    /// append or sync the record.
    pub(crate) fn log_definition(&mut self, tx: Option<&Txn>, change: Change) -> Result<()> {
        let Some(log) = &self.log else { return Ok(()) };
        self.tx.taking_changes()?;
        let record = Record { tx: tx.map_or(NO_TRANSACTION, |t| t.id), change }.encode();
        let lsn = log.append(&record)?;
        self.tx.sync(&**log, lsn)
    }
}

impl Txn {
    /// Whether its log keeps records across a crash.
    fn durable_log(&self) -> bool {
        self.log.as_ref().is_some_and(|log| log.capabilities().durable)
    }
}

impl State {
    /// Before a write to `table`, whose engine's data survives a crash as
    /// `persistence` says: what the `Strict` policy refuses (RULES.md D25).
    /// A `Forced` engine's writes need a durable log, to undo a transaction
    /// a crash interrupted; a `None` engine's writes are refused when the log
    /// is durable, since its data could not be; and a transaction writing an
    /// `External` engine's table writes no other, since the database cannot
    /// commit both atomically. A temporary table's instance keeps nothing
    /// across a crash, as it should [ISO/IEC 9075:1992 §4.9].
    pub(super) fn check_persistence(&self, tx: &Txn, table: &QualifiedName, persistence: Persistence) -> Result<()> {
        if self.catalog.table(table).is_some_and(|t| t.temporary.is_some()) {
            return Ok(());
        }
        let external = persistence == Persistence::External;
        let mut st = tx.state()?;
        if self.policy() == Policy::Strict {
            let durable = tx.durable_log();
            let mixed = st.first_write_external.is_some_and(|first| first != external);
            let why = if persistence == Persistence::Forced && !durable {
                "its engine makes its writes durable itself, but the log is not durable, so a transaction a crash interrupts could not be undone"
            } else if persistence == Persistence::None && durable {
                "its engine keeps nothing across a crash, but the log is durable, so its data could not be"
            } else if mixed {
                "the transaction writes a table of an engine with its own durability and a table of another, which cannot commit atomically"
            } else {
                ""
            };
            if !why.is_empty() {
                return Err(SqlError::new(
                    Code::Policy,
                    format!("[X3.135-1986 §4.16] {table} cannot be written: {why}, and the transaction policy is Strict"),
                ));
            }
        }
        // Recorded only once the write is allowed.
        st.first_write_external.get_or_insert(external);
        Ok(())
    }

    /// Whether changes to a table of an engine of `persistence` are logged:
    /// those the database recovers, redoing or undoing them.
    pub(crate) fn logs_changes(persistence: Persistence) -> bool {
        matches!(persistence, Persistence::Logged | Persistence::Forced)
    }
}
