//! The ends of transactions [X3.135-1986 §8.2, §8.9]: commit, two-phase
//! across the participants at one timestamp, or at the snapshot for a
//! transaction that wrote nothing; and rollback (RULES.md D25, D29). The
//! garbage collection of versions that follows either is `collect.rs`'s.

use std::collections::BTreeSet;
use std::mem;
use std::sync::atomic::Ordering;

use super::{Committing, Joined, Timestamp, Transactions, Txn, UndoFailure};
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::error::{Code, Result, SqlError, SqlState};
use crate::memory::Undoing;
use crate::sync::lock_builtin;

impl State {
    /// [X3.135-1986 §8.2 GR1, GR3] terminate the transaction and commit its
    /// changes: two-phase across the participants, at one commit timestamp.
    /// Between the phases its commit record is written; its locks are
    /// released, and then the record is synced, the moment it commits
    /// (RULES.md D25, D441). It returns once its commit is durable, so its
    /// session's next snapshot counts it. If a participant refuses, or the
    /// commit record cannot be written, the whole transaction is rolled back
    /// instead.
    ///
    /// # Errors
    ///
    /// As for rollback, and SQLCODE -922 if the commit record was written
    /// but it, or one before it, could not be synced: the transaction is in
    /// doubt, never durable, so no snapshot counts it, for the caller to
    /// keep until the database is dropped; recovery commits it if the log
    /// holds its commit record (RULES.md D25).
    pub(crate) fn commit(&self, tx: &Txn) -> Result<()> {
        self.commit_publishing(tx, &mut |_| Ok(()))
    }

    /// `commit`, running `publish` with the commit timestamp once the
    /// transaction has committed and
    /// before any snapshot counts it, still under the gate: so its
    /// definitions are published with its rows (`db/ddl.rs`). It is not run
    /// if the transaction rolls back instead.
    ///
    /// # Errors
    ///
    /// As for `commit`; and the error of `publish`, the transaction having
    /// committed.
    pub(crate) fn commit_publishing(&self, tx: &Txn, publish: &mut dyn FnMut(Timestamp) -> Result<()>) -> Result<()> {
        // A load under way is ended before it commits (RULES.md D532).
        self.end_load(tx)?;
        let (joined, wrote) = {
            let mut st = tx.state()?;
            // [DESIGN] RULES.md D922: its participants are taken, not
            // copied. One that wrote is given them back for its rollback
            // if the commit fails; one that read alone is done with them.
            (mem::take(&mut st.joined), st.wrote)
        };
        if !wrote {
            return Self::commit_read(tx, &joined);
        }
        // The commit record and the versions stamped are one change to a
        // checkpoint.
        let gate = self.tx.gate.enter();
        // Under the gate: an index published shut is either seen here, or
        // published after this commit, which it then counts.
        let validated = tx.check_committable().and_then(|()| self.validate_participants(tx, &joined)).and_then(|mut committing| {
            let now = self.clock.now();
            let lsn = tx.append_commit(committing.at, now)?;
            if lsn.is_some() {
                committing.syncs();
            }
            Ok((committing, (lsn, now)))
        });
        match validated {
            Ok((committing, (lsn, now))) => {
                // Phase 2 uses storages already found, so it cannot fail part
                // way. Each engine makes the changes committed at once, as
                // the row store does with one store (RULES.md D440).
                for participant in &joined {
                    participant.storage.commit(tx.id, committing.at);
                }
                // When each table it wrote last changed (RULES.md D522).
                tx.stamp_written(now);
                let at = committing.at;
                let published = publish(at);
                // The commit has reached every engine: it has ended, and is
                // durable once its record is synced.
                drop(committing);
                // [DESIGN] RULES.md D441: its locks go before the sync, so
                // the next writer of its rows goes on at once.
                tx.release();
                // [DESIGN] RULES.md D440: written in place once its locks
                // are released, and before the gate lets a checkpoint
                // capture the pages, whose record syncs this one's.
                for participant in &joined {
                    participant.storage.settle(tx.id);
                }
                drop(gate);
                let durable = self.durable(tx, lsn, at);
                self.collect_ended(tx, &joined);
                durable.and(published)
            }
            Err(e) => {
                drop(gate);
                tx.state()?.joined = joined;
                self.rollback(tx)?;
                // The transaction was rolled back: a conflict is a
                // serialization failure [ISO/IEC 9075:1992 §22.1].
                tx.replied(Err(if e.code == Code::Conflict { e.with_state(SqlState::SERIALIZATION_FAILURE) } else { e }))
            }
        }
    }

    /// \[DESIGN\] The commit of `tx`, which changed nothing, so it read one
    /// committed state, its snapshot, and is serializable there: no engine
    /// need check its reads, and it takes no commit timestamp (RULES.md
    /// D29).
    fn commit_read(tx: &Txn, joined: &[Joined]) -> Result<()> {
        for participant in joined {
            participant.storage.commit(tx.id, tx.snapshot());
        }
        tx.release();
        // [DESIGN] RULES.md D441: what it read past its snapshot is durable
        // before it says it committed.
        tx.replied(Ok(()))
    }

    /// \[DESIGN\] Syncs the commit record of `tx`, committed at `at`, at
    /// `lsn` if it has one to sync, and returns once every commit up to `at`
    /// is durable: not before every commit before it is, so its session's
    /// next snapshot counts it (RULES.md D29, D441).
    ///
    /// # Errors
    ///
    /// SQLCODE -922 if its sync, or one before, failed: it is in doubt.
    fn durable(&self, tx: &Txn, lsn: Option<crate::log::Lsn>, at: Timestamp) -> Result<()> {
        if let Some(lsn) = lsn {
            // Marked durable by its group's sync (`txn/group.rs`).
            tx.sync_commit(lsn, at)?;
        }
        self.tx.wait_durable(at)
    }

    /// Phase 1: takes the commit timestamp, and asks every participant in
    /// `joined` to validate at it. Nothing has committed if this fails.
    fn validate_participants(&self, tx: &Txn, joined: &[Joined]) -> Result<Committing<'_>> {
        let committing = self.tx.commit_timestamp();
        let at = committing.at;
        for Joined { table, storage } in joined {
            if let Err(e) = storage.validate(tx.id, at) {
                // [DESIGN] RULES.md D441: the refusal rests on commits
                // before it, told once they are durable.
                if e.code == Code::Conflict {
                    tx.depend(at.saturating_sub(1));
                }
                // A conflict keeps its code, so the program can tell it to
                // run the transaction again (RULES.md D29).
                let code = if e.code == Code::Conflict { Code::Conflict } else { Code::CommitFailed };
                return Err(SqlError::new(
                    code,
                    format!(
                        "[X3.135-1986 §8.2] the engine of {table} refused to commit, so the transaction was rolled back: {}",
                        e.described()
                    ),
                ));
            }
        }
        Ok(committing)
    }

    /// [X3.135-1986 §8.9 GR1, GR3] cancel every change and terminate the transaction.
    ///
    /// # Errors
    ///
    /// SQLCODE -914 if a change could not be undone. The transaction still ends.
    /// SQLCODE -922 if a commit it rests on was left in doubt by a failed sync.
    pub(crate) fn rollback(&self, tx: &Txn) -> Result<()> {
        // Never refused by the memory budget, nor the collection after,
        // which gives back what the undo leaves (RULES.md D38).
        let _undoing = Undoing::begin();
        // Undo first, then release the locks. A damaged table is marked before
        // its locks are released, so no other transaction sees it partly undone.
        let gate = self.tx.gate.enter();
        let (undo, joined) = {
            let mut st = tx.state()?;
            (st.undo.take_from(0), mem::take(&mut st.joined))
        };
        // A run of entries that cannot be read back fails the tables it
        // may have changed, the rest undone all the same.
        let mut failures = match undo {
            Ok(undo) => undo.apply(tx.id),
            Err(error) => vec![UndoFailure { table: None, error, rows: true }],
        };
        for Joined { table, storage } in &joined {
            if let Err(error) = storage.rollback(tx.id) {
                failures.push(UndoFailure { table: Some(QualifiedName::clone(table)), error, rows: true });
            }
        }
        let damage = self.report_damage(&failures, "the transaction's rollback");
        tx.log_abort();
        drop(gate);
        tx.release();
        self.collect_ended(tx, &joined);
        // [DESIGN] RULES.md D850: a completion told only as the
        // transaction ends, rolled back, waits as it would have.
        tx.replied(damage)
    }
}

impl Transactions {
    /// Notes that `table`'s engine has maintenance due, for the
    /// database's maintenance thread (`db/maintenance.rs`).
    pub(crate) fn note_maintenance(&self, table: &QualifiedName) {
        let mut noted = lock_builtin(&self.maintenance);
        // Asked first: a table noted already is not copied again.
        if !noted.contains(table) {
            noted.insert(table.clone());
        }
        self.noted.store(true, Ordering::Release);
    }

    /// Whether a table has maintenance due, looked at without a lock, as
    /// every transaction's end looks.
    pub(crate) fn maintenance_due(&self) -> bool {
        self.noted.load(Ordering::Acquire)
    }

    /// The tables noted with maintenance due, forgotten.
    pub(crate) fn take_maintenance(&self) -> BTreeSet<QualifiedName> {
        let mut noted = lock_builtin(&self.maintenance);
        self.noted.store(false, Ordering::Release);
        mem::take(&mut *noted)
    }

    /// Notes that `table`, of an assertion's kept totals, has rows to fold,
    /// for the database's job folding them (`db/assertions/fold.rs`).
    pub(crate) fn note_fold(&self, table: &QualifiedName) {
        lock_builtin(&self.folds).insert(table.clone());
    }

    /// Whether a table of kept totals has rows to fold.
    pub(crate) fn folds_due(&self) -> bool {
        !lock_builtin(&self.folds).is_empty()
    }

    /// The tables noted with rows to fold, forgotten.
    pub(crate) fn take_folds(&self) -> BTreeSet<QualifiedName> {
        mem::take(&mut *lock_builtin(&self.folds))
    }
}
