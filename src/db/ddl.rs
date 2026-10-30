//! \[EXTENSION\] SQL-schema statements in transactions [ISO/IEC 9075:1992
//! §4.28]: ALTER TABLE [§11.10] (`db/alter.rs`); DROP SCHEMA, TABLE, VIEW,
//! DOMAIN and ASSERTION [§11.2, §11.18, §11.20, §11.27, §11.35]
//! (`db/drop.rs`); a `<schema definition>`
//! (`db/staged_schema.rs`); and the CREATE, GRANT and REVOKE statements of
//! `db/definitions.rs`. Split from db.rs (RULES.md R-SIZE-2).
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

use std::sync::Arc;

use super::alter::{Made, Reshape};
use super::defaults::default_value;
use super::integrity::Effects;
use super::{Database, Session, State, api};
use crate::ast::{AlterAction, TableName};
use crate::binder::qualify;
use crate::catalog::{QualifiedName, TemporaryKind};
use crate::db::Completion;
use crate::error::{At, Code, Result, SqlError, fail};
use crate::index::Index;
use crate::log::record::Change;
use crate::parser::Parser;
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
            Change::AlterTable { table, action, schema, zone } => {
                let action = Parser::new(action)?.of_definition().alter_table_action()?;
                Ok(self.alter_table((table, schema, *zone), &action, (made, None))?.0)
            }
            Change::Defined { auth, schema, zone, sql, placed } => self.redefine((auth, schema, *zone), sql, placed.as_ref(), made),
            Change::Schema { sql, owner, engines, zone } => self.reschema((sql, owner, *zone), engines, made),
            Change::Insert { .. }
            | Change::Update { .. }
            | Change::Delete { .. }
            | Change::Commit { .. }
            | Change::Abort
            | Change::StatementUndone { .. }
            | Change::Checkpoint
            | Change::Stale { .. }
            | Change::CreateIndex { .. }
            | Change::DropIndex { .. }
            | Change::AlterIndex { .. }
            | Change::Setting { .. }
            | Change::User { .. }
            | Change::Owner { .. } => Err(api(format!("[DESIGN] {change:?} is not a change a transaction makes"))),
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
                Change::AlterTable { table, .. } => vec![table.clone()],
                Change::Defined { schema, sql, placed: Some(_), .. } => super::definitions::created(schema, sql).into_iter().collect(),
                Change::Schema { sql, engines, .. } => super::staged_schema::schema_tables(sql, engines),
                Change::Insert { .. }
                | Change::Update { .. }
                | Change::Delete { .. }
                | Change::Commit { .. }
                | Change::Abort
                | Change::StatementUndone { .. }
                | Change::Checkpoint
                | Change::Stale { .. }
                | Change::Defined { placed: None, .. }
                | Change::CreateIndex { .. }
                | Change::DropIndex { .. }
                | Change::AlterIndex { .. }
                | Change::Drop { .. }
                | Change::Setting { .. }
                | Change::User { .. }
                | Change::Owner { .. } => Vec::new(),
            };
            for table in renewed {
                self.lock_of(&table)?.replaced(self.generation_of(&table), at);
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
        self.prepare_change(st, tx, &qn)?;
        // [DESIGN] Of a temporary table, only while no session's instance
        // holds a row (RULES.md D49).
        if temporary.is_some() {
            st.temporaries.alter(&qn, tx.id)?;
        }
        let mut next = State::clone(st);
        let zone = self.now.get().zone;
        let (_, made, reshape) = next.alter_table((&qn, &schema, zone), action, (None, Some(tx)))?;
        let next = Arc::new(next);
        let checked = match (reshape, action) {
            (Some(reshape), _) => copy_rows(st, &next, tx, &qn, (action, reshape, self.effects(&next))),
            // [§11.12 GR1] a default changes no row, nor does an option
            // changed in place.
            (None, AlterAction::AlterColumn { .. } | AlterAction::Storage(_)) => Ok(()),
            (None, _) => validate_rows(&next, tx, &qn, self.effects(&next)),
        };
        if let Err(e) = checked {
            made.discard();
            return Err(e);
        }
        let change = Change::AlterTable { table: qn, action: text.to_owned(), schema, zone };
        self.stage(st, tx, Changed { next, change, made: Some(made) })?;
        Ok(super::long_texts::warned(Completion::Success, text, || super::long_texts::of_alter(action)))
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
    /// locks; the definitions logged, so a checkpoint keeps them.
    ///
    /// # Errors
    ///
    /// As for a commit; a change that cannot be made then, the transaction
    /// having committed (SQLCODE -910).
    pub(super) fn commit_staged(&self, tx: &Txn, staged: &Staged) -> Result<()> {
        let current = self.snapshot()?;
        let mut taken = Vec::new();
        let committed = current.commit_publishing(tx, &mut |at| {
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

/// Copies the rows of `table` that transaction `tx` sees in `st` into its
/// new generation in `next`, each reshaped as the action says, by `tx`'s
/// own writes.
///
/// # Errors
///
/// SQLCODE -201 if a column added NOT NULL would be null in a row
/// [ISO/IEC 9075:1992 §11.11 GR2]; the reads' and writes'.
fn copy_rows(
    st: &Arc<State>,
    next: &Arc<State>,
    tx: &Txn,
    table: &QualifiedName,
    (action, reshape, mut effects): (&AlterAction, Reshape, Effects<'_>),
) -> Result<()> {
    st.read_lock(tx, table)?;
    let width = st.catalog.table(table).map_or(0, |t| t.columns.len());
    let columns: Vec<usize> = (0..width).collect();
    // [§11.11 GR2] every value of a column added is its default.
    let added = match action {
        AlterAction::AddColumn(_) => next.catalog.table(table).and_then(|t| t.columns.last()).cloned(),
        AlterAction::AlterColumn { .. }
        | AlterAction::DropColumn { .. }
        | AlterAction::AddConstraint(_)
        | AlterAction::DropConstraint { .. }
        | AlterAction::Storage(_) => None,
    };
    let fill = added.as_ref().map(|c| default_value(c, effects.user(), effects.now)).transpose()?;
    for read in Arc::clone(st.storage_of(table)?).scan(tx.id, &columns) {
        let (_, row) = read?;
        let mut new = reshape.row(row);
        if let (Some(c), Some(value)) = (&added, &fill) {
            if c.not_null && value.is_null() {
                return fail(
                    Code::Constraint,
                    format!("[ISO/IEC 9075:1992 §11.11 GR2] {table} has rows, in which NOT NULL column {} would be null", c.name),
                );
            }
            if let Some(slot) = new.last_mut() {
                *slot = value.clone();
            }
        }
        effects.inserted(table, next.insert_row(tx, table, &new)?)?;
    }
    // The table's constraints, those of a column added among them, hold of
    // the rows copied.
    next.end_statement(tx, effects)
}

/// [ISO/IEC 9075:1992 §11.16 GR2] The rows of `table` transaction `tx`
/// sees, in `next`, its definitions with a constraint added, satisfy its
/// constraints: NOT NULL too, a PRIMARY KEY's columns not null [§11.7
/// SR3a].
pub(super) fn validate_rows(next: &Arc<State>, tx: &Txn, table: &QualifiedName, mut effects: Effects<'_>) -> Result<()> {
    next.read_lock(tx, table)?;
    let desc = next.catalog.table(table).cloned().ok_or_else(|| api(format!("[DESIGN] no table {table}")))?;
    let columns: Vec<usize> = (0..desc.columns.len()).collect();
    for read in Arc::clone(next.storage_of(table)?).scan(tx.id, &columns) {
        let (id, _) = read?;
        effects.inserted(table, id)?;
    }
    next.end_statement(tx, effects)
}
