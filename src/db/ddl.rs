//! \[EXTENSION\] SQL-schema statements in transactions [ISO/IEC 9075:1992
//! §4.28]: ALTER TABLE [§11.10] (`db/alter.rs`); DROP SCHEMA, TABLE, VIEW,
//! DOMAIN and ASSERTION [§11.2, §11.18, §11.20, §11.27, §11.35]
//! (`db/drop.rs`); a `<schema definition>` (`db/staged_schema.rs`); and
//! the CREATE, GRANT and REVOKE statements of `db/definitions.rs`.
//!
//! A transaction's changes of definitions are its own until it commits
//! (RULES.md D27): its statements see the published definitions with its
//! changes made on a copy (`Staged`), made again on a newer copy when
//! others publish meanwhile; others see none of them. Each change takes its
//! table's exclusive lock, held to the end, so no other transaction
//! changes that table or its definitions meanwhile, and none of its writers
//! is left open. A change is logged under the transaction, and recovery
//! makes it at the transaction's commit record (`db/recovery.rs`).
//!
//! COMMIT publishes the changes with the transaction's rows, before any
//! snapshot counts it (`State::commit_publishing`); ROLLBACK forgets them.
//! A dropped table's storage, and its indexes', are discarded once no
//! snapshot holds them (`TableStorage::discard`, `Index::discard`).
//! Whether SQL-schema and SQL-data statements may share a transaction is
//! implementation-defined [§4.28]: they may. A schema statement is refused
//! while one of the transaction's cursors reads its table, the effect on an
//! open cursor being implementation-defined too [§4.28].

mod copied;

use std::cell::Cell;
use std::sync::Arc;

use copied::emptied_rows;
pub(super) use copied::{copy_rows, validate_rows};

use super::alter::{Made, renamed_by};
use super::collations::added_collation;
use super::generated::Column;
use super::sequences::Epochs;
use super::{Database, Session, State, api};
use crate::ast::{AlterAction, AlterColumnAction, DropBehavior, TableName};
use crate::binder::{making_typed, qualify};
use crate::catalog::{QualifiedName, Temporary, TemporaryKind};
use crate::db::Completion;
use crate::error::{At, Code, Result, SqlError, fail};
use crate::index::Index;
use crate::log::record::Change;
use crate::parser::Parser;
use crate::settings::{FUNCTION_CALLS, XQUERY_STATIC_TYPING};
use crate::storage::TableStorage;
use crate::txn::{LockMode, Timestamp, Txn};

/// The definitions a transaction changed, not yet published.
pub(super) struct Staged {
    /// What its statements see: the published definitions it was made
    /// from, with its changes.
    state: Arc<State>,
    /// The generation of the published snapshot `state` was made from.
    base: u64,
    /// Its changes, in order, each with what it made: the storage and
    /// indexes an ALTER TABLE gave its table, published as they are.
    changes: Vec<Change>,
    made: Vec<Option<Made>>,
}

impl Staged {
    /// Discards what its changes made: the transaction rolled back.
    pub(super) fn discard(&self) {
        self.made.iter().flatten().for_each(Made::discard);
    }

    /// How many changes it holds.
    pub(super) fn len(&self) -> usize {
        self.changes.len()
    }

    /// [ISO/IEC 9075-2:2023 §17.8 GR3d] Its changes from the `kept`th on
    /// cancelled, and the definitions made again on `published` with
    /// those before: what the cancelled ones made, to be discarded.
    ///
    /// # Errors
    ///
    /// Making a change again fails, as `Session::definitions` does.
    pub(super) fn cancel_from(&mut self, kept: usize, published: &Arc<State>) -> Result<Vec<Made>> {
        self.changes.truncate(kept);
        let cancelled = self.made.split_off(kept.min(self.made.len()));
        let mut next = State::clone(published);
        for (change, made) in self.changes.iter().zip(&self.made) {
            next.apply_change(change, made.as_ref())?;
        }
        self.state = Arc::new(next);
        self.base = published.generation;
        Ok(cancelled.into_iter().flatten().collect())
    }
}

/// A change of definitions a statement made: the definitions with it, the
/// change, as logged, and what it made.
pub(super) struct Changed {
    pub(super) next: Arc<State>,
    pub(super) change: Change,
    pub(super) made: Option<Made>,
}

/// What a change of definitions took away, to discard once published.
#[derive(Default)]
pub(super) struct Dropped {
    pub(super) storage: Vec<Arc<dyn TableStorage>>,
    pub(super) indexes: Vec<Arc<dyn Index>>,
    /// The names of what a drop took away, or changed the definitions
    /// of: those whose locks its transaction holds exclusive (`db/drop.rs`).
    pub(super) names: Vec<QualifiedName>,
}

impl Dropped {
    /// Takes in what `other` took away.
    pub(super) fn absorb(&mut self, other: Dropped) {
        self.storage.extend(other.storage);
        self.indexes.extend(other.indexes);
        self.names.extend(other.names);
    }

    /// Discards what was taken away: freed once no snapshot holds it.
    pub(super) fn discard(self) {
        self.storage.iter().for_each(|s| s.discard());
        self.indexes.iter().for_each(|i| i.discard());
    }
}

impl State {
    /// Makes `change`, a change of definitions a transaction made, with
    /// what its statement made, if given, else anew: what it took away.
    /// The checks of its statement were made when it ran.
    ///
    /// # Errors
    ///
    /// The change's own; SQLCODE -910 if it is not one a transaction makes.
    pub(super) fn apply_change(&mut self, change: &Change, made: Option<&Made>) -> Result<Dropped> {
        match change {
            Change::Drop { object, cascade } => self.drop_object(object, *cascade),
            Change::AlterTable { table, action, schema, zone, epochs, typed } => {
                let _typed = making_typed(*typed);
                let action = Parser::for_stored(action)?.alter_table_action()?;
                Ok(self.alter_table((table, schema, *zone), &action, (made, None, &mut Epochs::logged(epochs)))?.0)
            }
            Change::Defined { auth, role, schema, now, sql, placed, epochs, typed } => {
                let _typed = making_typed(*typed);
                self.redefine(((auth, role.as_deref()), schema, *now), sql, (placed.as_ref(), made, epochs))
            }
            Change::Schema { sql, owner, engines, now, epochs, typed } => {
                let _typed = making_typed(*typed);
                self.reschema((sql, owner, *now), engines, made, epochs)
            }
            Change::Spatial(c) => {
                let next = self.catalog.spatial().applied(c)?;
                let mut catalog = self.catalog.duplicate();
                catalog.set_spatial(next);
                self.catalog = Arc::from(catalog);
                Ok(Dropped::default())
            }
            Change::Insert { .. }
            | Change::Update { .. }
            | Change::Delete { .. }
            | Change::Commit { .. }
            | Change::Abort
            | Change::StatementUndone { .. }
            | Change::Checkpoint
            | Change::RestorePoint { .. }
            | Change::Stale { .. }
            | Change::LobPut { .. }
            | Change::LobStale { .. }
            | Change::CreateIndex { .. }
            | Change::DropIndex { .. }
            | Change::AlterIndex { .. }
            | Change::Setting { .. }
            | Change::User { .. }
            | Change::Owner { .. }
            | Change::RoleCreator { .. }
            | Change::Sequence { .. }
            | Change::Reconfiguration { .. } => Err(api(format!("[DESIGN] {change:?} is not a change a transaction makes"))),
        }
    }
}

impl State {
    /// Makes `staged`'s changes, of a transaction that committed at `at`,
    /// with what they made, and records them among the definitions: what
    /// they took away.
    fn publish(&mut self, staged: &Staged, at: Timestamp) -> Result<Vec<Dropped>> {
        let mut dropped = Vec::new();
        for (change, made) in staged.changes.iter().zip(&staged.made) {
            dropped.push(self.apply_change(change, made.as_ref())?);
            // Statements and transactions that began before see it no more
            // (`TableLock::check_generation`): a table altered, or created,
            // whose rows their snapshots do not see.
            let renewed: Vec<QualifiedName> = match change {
                // [EXTENSION] of a table renamed, its new name too (RULES.md D402).
                Change::AlterTable { table, action, .. } => renamed_by(table, action),
                Change::Defined { schema, sql, placed: Some(_), .. } => super::definitions::created(schema, sql).into_iter().collect(),
                Change::Schema { sql, engines, .. } => super::staged_schema::schema_tables(sql, engines),
                Change::Insert { .. }
                | Change::Update { .. }
                | Change::Delete { .. }
                | Change::Commit { .. }
                | Change::Abort
                | Change::StatementUndone { .. }
                | Change::Checkpoint
                | Change::RestorePoint { .. }
                | Change::Stale { .. }
                | Change::LobPut { .. }
                | Change::LobStale { .. }
                | Change::Defined { placed: None, .. }
                | Change::CreateIndex { .. }
                | Change::DropIndex { .. }
                | Change::AlterIndex { .. }
                | Change::Drop { .. }
                | Change::Setting { .. }
                | Change::User { .. }
                | Change::Owner { .. }
                | Change::RoleCreator { .. }
                | Change::Sequence { .. }
                | Change::Spatial(_)
                | Change::Reconfiguration { .. } => Vec::new(),
            };
            for table in renewed {
                self.lock_of(&table)?.replaced(self.generation_of(&table), at);
                // [DESIGN] each session's instance of a temporary one, too
                // (RULES.md D49).
                if self.catalog.table(&table).is_some_and(|t| t.temporary.is_some()) {
                    self.temporaries.published(&table, self.generation_of(&table), at);
                }
            }
        }
        Ok(dropped)
    }
}

impl Session {
    /// The definitions a call of this session sees: `published`, or its
    /// transaction's, made again on `published` if that is newer.
    ///
    /// # Errors
    ///
    /// Making a change again fails: the call fails, and the transaction can
    /// only roll back.
    pub(super) fn definitions(&self, published: Arc<State>) -> Result<Arc<State>> {
        let mut inner = self.inner.borrow_mut();
        let Some(staged) = inner.staged.as_mut() else { return Ok(published) };
        if staged.base != published.generation {
            let mut next = State::clone(&published);
            for (change, made) in staged.changes.iter().zip(&staged.made) {
                next.apply_change(change, made.as_ref())?;
            }
            staged.state = Arc::new(next);
            staged.base = published.generation;
        }
        Ok(Arc::clone(&staged.state))
    }

    /// The transaction's changes of definitions, taken away: its end.
    pub(super) fn take_staged(&self) -> Option<Staged> {
        self.inner.borrow_mut().staged.take()
    }

    /// [ISO/IEC 9075:1992 §11.10] ALTER TABLE, in transaction `tx`, its
    /// action written as `text` (`db/alter.rs`): for a column added or
    /// dropped, or storage rebuilt, the table given a new generation of
    /// its storage and its rows copied there; for a constraint added, its
    /// rows checked against it.
    pub(super) fn alter_table(
        &self,
        st: &Arc<State>,
        tx: &Txn,
        (table, action, text): (&TableName, &AlterAction, &str),
    ) -> Result<Completion> {
        // Its own texts read again as the session read it, so a CHECK or
        // a generation expression it adds takes calls of registered
        // functions only as the session does (RULES.md D74).
        let _calls = Parser::refusing_calls(text, st.settings.is_on(FUNCTION_CALLS));
        let schema = self.default_schema();
        let qn = qualify(table, &schema);
        // [AR1] only the owner of its schema alters a table.
        if !self.owns_schema(st, &qn.auth) {
            return fail(Code::Privilege, format!("[ISO/IEC 9075:1992 §11.10 AR1] {qn} is in a schema {} does not own", self.auth()));
        }
        let temporary = st.base_table(&qn).or_at(table.at)?.temporary;
        if temporary.is_some_and(|t| t.kind == TemporaryKind::Declared) {
            return fail(
                Code::SyntaxRule,
                format!("[ISO/IEC 9075:1992 §11.10 SR5] {qn} is a declared local temporary table: it is not altered"),
            );
        }
        // [ISO/IEC 9075-2:2023 §11.23 GR3] the generated columns on a column
        // dropped with CASCADE dropped first; the transaction only rolls
        // back if the drop then fails.
        let AlterAction::DropColumn { column, behavior: DropBehavior::Cascade, .. } = action else {
            return self.alter_base_table(st, tx, (&qn, action, text), temporary);
        };
        let desc = st.base_table(&qn).or_at(table.at)?;
        let on = match desc.column(column) {
            Some(at) => st.generated_on(&desc, at)?,
            None => Vec::new(),
        };
        let lost: Vec<Column> = on.into_iter().map(|name| (qn.clone(), name)).collect();
        self.dropping_generations(tx, &lost, || {
            let fresh = if lost.is_empty() { Arc::clone(st) } else { self.definitions(self.db.snapshot()?)? };
            self.alter_base_table(&fresh, tx, (&qn, action, text), temporary)
        })
    }

    /// `alter_table` of table `qn`, temporary as `temporary` says, without
    /// its Access Rule: as a collation's columns are reverted, whoever owns
    /// them [ISO/IEC 9075-2:2023 §11.44 GR2, §12.7 GR12ad].
    pub(super) fn alter_base_table(
        &self,
        st: &Arc<State>,
        tx: &Txn,
        (qn, action, text): (&QualifiedName, &AlterAction, &str),
        temporary: Option<Temporary>,
    ) -> Result<Completion> {
        let schema = self.default_schema();
        self.prepare_altered(st, tx, (qn, action, &schema))?;
        // [DESIGN] Of a temporary table, no other transaction using an
        // instance of it meanwhile, nor one having written one since this
        // transaction's snapshot (RULES.md D49).
        if temporary.is_some() {
            st.temporaries.alter(qn, (tx.id, tx.snapshot()))?;
        }
        let mut next = State::clone(st);
        let zone = self.now.get().zone;
        // [XQuery §5.2.3] typed statically as the session asks, logged with it.
        let typed = st.settings.is_on(XQUERY_STATIC_TYPING);
        let _typed = making_typed(typed);
        let mut epochs = Epochs::fresh();
        let (_, made, reshape) = next.alter_table((qn, &schema, zone), action, (None, Some(tx), &mut epochs))?;
        // A column added of a collation made holds its name shared, so no
        // REVOKE or DROP of it reverts the columns meanwhile.
        if let Some(Err(e)) = added_collation(&next, qn, action).map(|c| st.acquire_name(tx, &c, LockMode::Shared)) {
            made.discard();
            return Err(e);
        }
        let next = Arc::new(next);
        let checked = match (reshape, action) {
            // [DESIGN] Of a temporary table, each session's instance's rows,
            // not the definition's (`db/temporary/reshape.rs`).
            (Some(reshape), _) => match temporary {
                Some(_) => self.reshape_instances((st, &next), tx, (qn, action, reshape)),
                // [EXTENSION] the entries of a partition's rows of its old
                // storage taken from the global indexes over them, and those
                // of its new made as they are copied (RULES.md D157).
                None => st.unindex_partitions(tx, qn).and_then(|()| copy_rows(st, &next, tx, qn, (action, reshape, self.effects(&next)))),
            },
            // [ISO/IEC 9075-2:2023 §11.15 GR1d] the rows checked of the
            // NOT NULL constraint added.
            (None, AlterAction::AlterColumn { action: AlterColumnAction::SetNotNull, .. }) => match temporary {
                Some(_) => self.validate_instances((st, &next), tx, (qn, action)),
                None => validate_rows(&next, tx, qn, self.effects(&next)),
            },
            // [§11.12 GR1] a default changes no row, nor does an option
            // changed in place, nor a partition detached.
            (None, AlterAction::AlterColumn { .. } | AlterAction::Storage(_)) => Ok(()),
            // [EXTENSION] the rows detached out of the global indexes of
            // the tree they leave (RULES.md D157).
            (None, AlterAction::DetachPartition(table)) => st.unindex_partitions(tx, &qualify(table, &schema)),
            // [ISO/IEC 9075-2:2023 §14.10 GR5] Of a temporary table, the
            // session's rows deleted already (`db/truncate.rs`).
            (None, AlterAction::Truncate { .. }) => match temporary {
                Some(_) => Ok(()),
                None => st.unindex_partitions(tx, qn).and_then(|()| emptied_rows(&next, tx, qn, self.effects(&next))),
            },
            // [EXTENSION] the rows attached held by their bounds (RULES.md D153).
            (None, AlterAction::AttachPartition { table, .. }) => {
                let child = qualify(table, &schema);
                next.index_partitions(tx, &child)
                    .and_then(|()| super::partitions::validate_attached(&next, tx, &child, self.effects(&next)))
            }
            // [DESIGN] Of a temporary table, each session's rows, as it sees
            // them (`db/temporary/reshape.rs`).
            (None, _) => match temporary {
                Some(_) => self.validate_instances((st, &next), tx, (qn, action)),
                None => validate_rows(&next, tx, qn, self.effects(&next)),
            },
        };
        if let Err(e) = checked {
            made.discard();
            return Err(e);
        }
        // [DESIGN] a table renamed logged with its storage's generation (RULES.md D402).
        let logged = super::alter::logged_rename(&next, qn, action).unwrap_or_else(|| text.to_owned());
        let change = Change::AlterTable { table: qn.clone(), action: logged, schema, zone, epochs: epochs.given(), typed };
        self.stage(st, tx, Changed { next, change, made: Some(made) })?;
        Ok(super::long_texts::warned(Completion::Success, text, || super::long_texts::of_alter(action)))
    }

    /// Before ALTER TABLE `qn` `action`, its names of schema `schema`, in
    /// transaction `tx`: the exclusive lock of each table it changes, and
    /// of the name it gives one.
    fn prepare_altered(&self, st: &Arc<State>, tx: &Txn, (qn, action, schema): (&QualifiedName, &AlterAction, &str)) -> Result<()> {
        self.prepare_change(st, tx, qn)?;
        // [EXTENSION] the table attached or detached changed too (RULES.md D153).
        if let AlterAction::AttachPartition { table, .. } | AlterAction::DetachPartition(table) = action {
            self.prepare_change(st, tx, &qualify(table, schema))?;
        }
        // [EXTENSION] the name a table is given held (RULES.md D402).
        if let AlterAction::RenameTable { .. } = action {
            self.prepare_change(st, tx, &super::alter::renamed_to(qn, action))?;
        }
        Ok(())
    }

    /// Before a change of definitions to `table`, a table, view or schema
    /// (`QualifiedName::of_schema`), in transaction `tx`: no cursor of the
    /// transaction reads it, and its exclusive lock is held.
    pub(super) fn prepare_change(&self, st: &Arc<State>, tx: &Txn, table: &QualifiedName) -> Result<()> {
        if self.cursor_reads(table) {
            return fail(
                Code::Cursor,
                format!(
                    "[DESIGN] a cursor of this transaction reads {table}; close it first [ISO/IEC 9075:1992 §4.28, implementation-defined]"
                ),
            );
        }
        st.acquire_name(tx, table, LockMode::Exclusive)
    }

    /// Makes `changed.next` the definitions of transaction `tx`, its change
    /// logged.
    pub(super) fn stage(&self, st: &Arc<State>, tx: &Txn, changed: Changed) -> Result<()> {
        let Changed { next, change: logged, made } = changed;
        tx.log_change(logged.clone())?;
        tx.wrote_definitions()?;
        let mut inner = self.inner.borrow_mut();
        let base = inner.staged.as_ref().map_or(st.generation, |s| s.base);
        let mut staged = inner.staged.take().unwrap_or(Staged { state: Arc::clone(st), base, changes: Vec::new(), made: Vec::new() });
        staged.changes.push(logged);
        staged.made.push(made);
        staged.state = next;
        inner.staged = Some(staged);
        Ok(())
    }
}

impl Database {
    /// Commits `tx`, publishing `staged`, its changes of definitions, with
    /// it: made on the definitions published by then, which no other
    /// transaction changed for the tables these are of, as it held their
    /// locks; the definitions logged, so a checkpoint keeps them. Sets
    /// `committed` once it has committed, so a failure after is told from
    /// one before.
    ///
    /// # Errors
    ///
    /// As for a commit; a change that cannot be made then, the transaction
    /// having committed (SQLCODE -910).
    pub(super) fn commit_staged(&self, tx: &Txn, staged: &Staged, committed: &Cell<bool>) -> Result<()> {
        let current = self.snapshot()?;
        let mut taken = Vec::new();
        let committed = current.commit_publishing(tx, &mut |at| {
            committed.set(true);
            let dropped = self.change(|st| st.publish(staged, at));
            taken = dropped.map_err(|e| {
                SqlError::new(
                    Code::Api,
                    format!("[DESIGN] the transaction committed, but its definitions could not be published: {}", e.described()),
                )
            })?;
            Ok(())
        });
        taken.into_iter().for_each(Dropped::discard);
        committed
    }
}
