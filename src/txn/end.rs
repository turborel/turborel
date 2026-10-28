//! The ends of transactions [X3.135-1986 §8.2, §8.9]: commit, two-phase
//! across the participants at one timestamp, or at the snapshot for a
//! transaction that wrote nothing; rollback; and the garbage collection of
//! versions that follows either (RULES.md D25, D29). Split from txn.rs
//! (RULES.md R-SIZE-2).

use std::collections::BTreeSet;
use std::mem;

use super::{Committing, Joined, Timestamp, TxId, Txn, UndoFailure};
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::error::{Code, Result, SqlError, SqlState};
use crate::index::{Index, Key};
use crate::storage::{Row, RowId, TableStorage, project};
use crate::sync::lock_builtin;
use crate::value::Value;

impl State {
    /// [X3.135-1986 §8.2 GR1, GR3] terminate the transaction and commit its
    /// changes: two-phase across the participants, at one commit timestamp.
    /// Between the phases its commit record is made durable (RULES.md D25):
    /// that is the moment it commits. If a participant refuses, or the
    /// commit record cannot be written, the whole transaction is rolled back
    /// instead. Its locks are released at the end.
    ///
    /// # Errors
    ///
    /// As for rollback, and SQLCODE -915 if the commit record was written but
    /// could not be synced: the transaction is committed, but whether that
    /// survives a crash is unknown.
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
        let (joined, wrote) = {
            let st = tx.state()?;
            (st.joined.clone(), st.wrote)
        };
        if !wrote {
            // \[DESIGN\] It changed nothing, so it read one committed state,
            // its snapshot, and is serializable there: no engine need check
            // its reads, and it takes no commit timestamp (RULES.md D29).
            for participant in &joined {
                participant.storage.commit(tx.id, tx.snapshot());
            }
            tx.state()?.joined.clear();
            tx.release();
            self.gc(&tables(&joined));
            return Ok(());
        }
        // The commit record and the versions stamped are one change to a
        // checkpoint.
        let gate = self.tx.gate.enter();
        // Under the gate: an index published shut is either seen here, or
        // published after this commit, which it then counts.
        let validated =
            tx.check_definitions().and_then(|()| tx.check_undone()).and_then(|()| self.validate_participants(tx.id, &joined)).and_then(
                |(participants, committing)| {
                    let synced = tx.commit_record(committing.at)?;
                    Ok((participants, committing, synced))
                },
            );
        match validated {
            Ok((participants, committing, synced)) => {
                // Phase 2 uses storages already found, so it cannot fail part way.
                for storage in participants {
                    storage.commit(tx.id, committing.at);
                }
                let published = publish(committing.at);
                // The commit has reached every engine: snapshots may now
                // count it.
                drop(committing);
                drop(gate);
                tx.release();
                self.gc(&tables(&joined));
                published.and(synced)
            }
            Err(e) => {
                drop(gate);
                self.rollback(tx)?;
                // The transaction was rolled back: a conflict is a
                // serialization failure [ISO/IEC 9075:1992 §22.1].
                Err(if e.code == Code::Conflict { e.with_state(SqlState::SERIALIZATION_FAILURE) } else { e })
            }
        }
    }

    /// Phase 1: takes the commit timestamp, and asks every participant in
    /// `joined` to validate at it. Nothing has committed if this fails.
    fn validate_participants<'j>(&self, tx: TxId, joined: &'j [Joined]) -> Result<(Vec<&'j dyn TableStorage>, Committing<'_>)> {
        let committing = self.tx.commit_timestamp();
        let at = committing.at;
        for Joined { table, storage } in joined {
            if let Err(e) = storage.validate(tx, at) {
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
        Ok((joined.iter().map(|j| &*j.storage).collect(), committing))
    }

    /// After a transaction ends, garbage collection: each of `tables`, and
    /// each table whose versions still awaited collection when it was last
    /// collected, removes the versions no open transaction can read, and
    /// their index entries go with them (RULES.md D29). The horizon moves
    /// only as a transaction ends, so a table no transaction touches is
    /// collected all the same, as soon as it can be. A failure leaves them
    /// for the next transaction to end: the one that ended has already
    /// committed or rolled back.
    pub(crate) fn gc(&self, tables: &[QualifiedName]) {
        let _gate = self.tx.gate.enter();
        let horizon = self.tx.horizon();
        let waiting: Vec<QualifiedName> = lock_builtin(&self.tx.uncollected).iter().cloned().collect();
        let all: BTreeSet<&QualifiedName> = tables.iter().chain(&waiting).collect();
        for table in all {
            let left = self.collect_table(table, horizon);
            let mut uncollected = lock_builtin(&self.tx.uncollected);
            match left {
                Some(true) => uncollected.insert(table.clone()),
                Some(false) | None => uncollected.remove(table),
            };
        }
    }

    /// Collects `table`'s garbage past `horizon`: whether versions of it
    /// still await collection, or none if it is not there.
    fn collect_table(&self, table: &QualifiedName, horizon: Timestamp) -> Option<bool> {
        // Not while an index of it is built, which counts every version it
        // finds; nor with definitions older than its indexes, whose entries
        // for the versions removed would stay. A later transaction collects
        // them.
        let lock = self.lock_of(table).ok()?;
        let storage = self.storage_of(table).ok()?;
        if lock.collection_held() || lock.changed_after(self.generation) {
            return Some(true);
        }
        {
            let indexes: Vec<(&dyn Index, &[usize])> = self
                .catalog
                .indexes(table)
                .into_iter()
                .filter_map(|ix| Some((self.index_of(&ix.name).ok()?, ix.columns.as_slice())))
                .collect();
            // Each version's entries go as it is removed. A failure leaves
            // the rest for a later call; an entry left behind only costs
            // readers a row they check and skip.
            let _ = storage.gc(horizon, &mut |id, row| remove_entries(&indexes, id, &row));
        }
        Some(storage.uncollected())
    }

    /// [X3.135-1986 §8.9 GR1, GR3] cancel every change and terminate the transaction.
    ///
    /// # Errors
    ///
    /// SQLCODE -914 if a change could not be undone. The transaction still ends.
    pub(crate) fn rollback(&self, tx: &Txn) -> Result<()> {
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
                failures.push(UndoFailure { table: Some(table.clone()), error, rows: true });
            }
        }
        let damage = self.report_damage(&failures, "the transaction's rollback");
        tx.log_abort();
        drop(gate);
        tx.release();
        self.gc(&tables(&joined));
        damage
    }
}

/// Removes the entries of row `id`, a version collected, of `row`, from
/// `indexes`, each with its columns. Keys with a null were never inserted
/// (`Index::insert`); one that cannot be removed is left.
fn remove_entries(indexes: &[(&dyn Index, &[usize])], id: RowId, row: &Row) {
    for (index, columns) in indexes {
        if let Some(key) = project(row, columns).map(Key).filter(|k| !k.0.iter().any(Value::is_null)) {
            let _ = index.remove(&key, id);
        }
    }
}

/// The tables of `joined`, for garbage collection.
fn tables(joined: &[Joined]) -> Vec<QualifiedName> {
    joined.iter().map(|j| j.table.clone()).collect()
}
