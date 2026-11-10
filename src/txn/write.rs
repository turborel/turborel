//! The row writes of the data-changing statements [X3.135-1986 §8.4, §8.5,
//! §8.7, §8.11, §8.12]: each changes a row and its index entries together, and
//! logs what undoes it for the coordinator [X3.135-1986 §4.16, §7.3 GR3b i].

use std::sync::Arc;

use super::UndoEntry;
use crate::catalog::{IndexDesc, QualifiedName};
use crate::db::State;
use crate::error::{Code, Result, SqlError};
use crate::index::{Index, Key};
use crate::log::record::Change;
use crate::storage::{Row, RowId};
use crate::txn::{Concurrency, LockWait, Timestamp, Txn, Undo};
use crate::value::Value;

impl State {
    fn table_indexes(&self, table: &QualifiedName) -> Vec<IndexDesc> {
        self.catalog.indexes(table).into_iter().cloned().collect()
    }

    /// The key of `row` in the index `ix`.
    fn index_key(ix: &IndexDesc, row: &Row) -> Result<Key> {
        ix.key_of(row).ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] a row is shorter than index {}", ix.name)))
    }

    fn index_add(&self, tx: &Txn, ix: &IndexDesc, row: &Row, id: RowId) -> Result<()> {
        let key = Self::index_key(ix, row)?;
        if !key.0.iter().any(Value::is_null) {
            let index = self.index_handle(&ix.name)?;
            // Of a unique index, whether the key's only entry, so its
            // constraint need not look for another row (`Txn::alone`).
            if ix.unique {
                let alone = index.insert_alone(key.clone(), id)?;
                tx.note_alone(&ix.name, id, alone)?;
            } else {
                index.insert(key.clone(), id)?;
            }
            tx.log(UndoEntry::IndexInserted { table: ix.table.clone(), index, key, id })?;
        }
        Ok(())
    }

    fn index_remove(&self, tx: &Txn, ix: &IndexDesc, row: &Row, id: RowId) -> Result<()> {
        let key = Self::index_key(ix, row)?;
        if !key.0.iter().any(Value::is_null) {
            let index = self.index_handle(&ix.name)?;
            index.remove(&key, id)?;
            tx.log(UndoEntry::IndexRemoved { table: ix.table.clone(), index, key, id })?;
        }
        Ok(())
    }

    /// The index named `name`, shared.
    fn index_handle(&self, name: &str) -> Result<Arc<dyn Index>> {
        self.indexes.get(name).map(Arc::clone).ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] the index {name} does not exist")))
    }

    /// [X3.135-1986 §8.7] Inserts `row`, with its index entries, under its
    /// table's lock (`prepare_write`), logging what undoes it [X3.135-1986
    /// §4.16, §7.3 GR3b i].
    pub(crate) fn insert_row(&self, tx: &Txn, table: &QualifiedName, row: &Row) -> Result<RowId> {
        self.prepare_write(tx, table)?;
        // After any wait for a lock: a checkpoint never waits on a waiter.
        let _gate = self.tx.gate.enter();
        let id = self.storage_of(table)?.insert(tx.id, row.clone())?;
        let caps = self.capabilities(table)?;
        if caps.undo == Undo::Log {
            tx.log(UndoEntry::Inserted { table: table.clone(), storage: Arc::clone(self.storage_of(table)?), id })?;
        }
        if Self::logs_changes(caps.persistence) {
            let change = Change::Insert { table: table.clone(), generation: self.generation_of(table), row: id, after: row.clone() };
            tx.log_sealed(change, self.seal_of(table)?.as_deref().map(|p| (p, id)))?;
        }
        for ix in self.table_indexes(table) {
            self.index_add(tx, &ix, row, id)?;
        }
        Ok(id)
    }

    /// [X3.135-1986 §8.11, §8.12] Replaces row `id` with `row`, as `insert_row`
    /// inserts, waiting for a transaction changing it to end (RULES.md D28).
    pub(crate) fn update_row(&self, tx: &Txn, table: &QualifiedName, id: RowId, row: &Row) -> Result<()> {
        self.change_row(tx, (table, id), Write::Update(row.clone()), None).map(|_| ())
    }

    /// [X3.135-1986 §8.4, §8.5] Deletes row `id`, as `update_row` replaces it.
    pub(crate) fn delete_row(&self, tx: &Txn, table: &QualifiedName, id: RowId) -> Result<()> {
        self.change_row(tx, (table, id), Write::Delete, None).map(|_| ())
    }

    /// As `update_row` with `row`, or `delete_row` with none; of a row
    /// another transaction changed after a READ COMMITTED statement's
    /// snapshot, its version committed then written over instead, as
    /// `recheck` says (RULES.md D29): the row it replaced, none if it was
    /// left alone.
    pub(crate) fn rewrite_row(
        &self,
        tx: &Txn,
        (table, id): (&QualifiedName, RowId),
        row: Option<&Row>,
        recheck: Recheck<'_>,
    ) -> Result<Option<Row>> {
        let write = row.map_or(Write::Delete, |row| Write::Update(row.clone()));
        self.change_row(tx, (table, id), write, Some(recheck))
    }

    /// `write` of row `id`, tried again as long as another transaction's
    /// change refuses it and LOCKS.WAIT lets it wait: once that
    /// transaction ends, over the version it committed, if `recheck`
    /// is given and the transaction reads at a snapshot for each statement
    /// [ISO/IEC 9075:1992 §4.28]. The row replaced, none if left alone.
    fn change_row(
        &self,
        tx: &Txn,
        (table, id): (&QualifiedName, RowId),
        mut write: Write,
        mut recheck: Option<Recheck<'_>>,
    ) -> Result<Option<Row>> {
        self.prepare_write(tx, table)?;
        let mut seen: Option<(Timestamp, Row)> = None;
        let mut tries = 0_usize;
        loop {
            let attempt = {
                // After any wait for a lock: a checkpoint never waits on a waiter.
                let _gate = self.tx.gate.enter();
                self.write_once(tx, (table, id), &write, seen.as_ref())
            };
            let e = match attempt {
                Ok(old) => return Ok(Some(old)),
                Err(e) => e,
            };
            tries = tries.saturating_add(1);
            let waits = e.code == Code::Conflict && tx.lock_wait == LockWait::Wait && tries < MOST_CONFLICTS;
            if !waits || self.capabilities(table)?.concurrency == Concurrency::Coordinator {
                return Err(e);
            }
            let newest = self.storage_of(table)?.newest(tx.id, id)?;
            if let Some(writer) = newest.writer {
                self.wait_for_writer(tx, table, writer)?;
                seen = None;
                continue;
            }
            // Its writer rolled back as this one looked.
            if newest.stamp <= tx.snapshot() {
                seen = None;
                continue;
            }
            // [§4.28] Committed since its one snapshot: what it read changed.
            let Some(check) = recheck.as_mut().filter(|_| !tx.one_snapshot()) else { return Err(e) };
            let Some(row) = newest.row else { return Ok(None) };
            write = match check(&row)? {
                Again::Update(row) => Write::Update(row),
                Again::Delete => Write::Delete,
                Again::Skip => return Ok(None),
            };
            seen = Some((newest.stamp, row));
        }
    }

    /// `write` of row `id` once, over the version `seen` if given, with
    /// its index entries and what undoes it: the row it replaced.
    fn write_once(&self, tx: &Txn, (table, id): (&QualifiedName, RowId), write: &Write, seen: Option<&(Timestamp, Row)>) -> Result<Row> {
        let storage = self.storage_of(table)?;
        let old = match seen {
            Some((_, row)) => row.clone(),
            None => storage.get(tx.id, id)?.ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] no row {id} in {table}")))?,
        };
        let new = match write {
            Write::Update(row) => Some(row.clone()),
            Write::Delete => None,
        };
        match (new, seen) {
            (new, Some((stamp, _))) => storage.write_newest(tx.id, id, new, *stamp)?,
            (Some(new), None) => storage.update(tx.id, id, new)?,
            (None, None) => storage.delete(tx.id, id)?,
        }
        match write {
            Write::Update(row) => self.updated(tx, (table, id), (&old, row))?,
            Write::Delete => self.deleted(tx, table, id, &old)?,
        }
        Ok(old)
    }

    /// Of row `id` of `table` made `row` from `old`: what undoes it, its
    /// log record and its index entries.
    fn updated(&self, tx: &Txn, (table, id): (&QualifiedName, RowId), (old, row): (&Row, &Row)) -> Result<()> {
        let storage = self.storage_of(table)?;
        let caps = self.capabilities(table)?;
        if caps.undo == Undo::Log {
            tx.log(UndoEntry::Updated { table: table.clone(), storage: Arc::clone(storage), id, old: old.clone() })?;
        }
        if Self::logs_changes(caps.persistence) {
            let change = Change::Update {
                table: table.clone(),
                generation: self.generation_of(table),
                row: id,
                before: old.clone(),
                after: row.clone(),
            };
            tx.log_sealed(change, self.seal_of(table)?.as_deref().map(|p| (p, id)))?;
        }
        // An engine keeping versions keeps the old version's entries for
        // the readers that still see it, until the version is removed
        // (`TableStorage::gc`); the new version gets entries of its own.
        let versions = caps.concurrency != Concurrency::Coordinator;
        for ix in self.table_indexes(table) {
            if !versions {
                self.index_remove(tx, &ix, old, id)?;
            }
            self.index_add(tx, &ix, row, id)?;
        }
        Ok(())
    }

    /// Of row `id` of `table`, `old`, deleted, as `updated` says.
    fn deleted(&self, tx: &Txn, table: &QualifiedName, id: RowId, old: &Row) -> Result<()> {
        let storage = self.storage_of(table)?;
        let caps = self.capabilities(table)?;
        // An engine keeping versions keeps the deleted version's entries
        // until it is removed, as for an update.
        if caps.concurrency == Concurrency::Coordinator {
            for ix in self.table_indexes(table) {
                self.index_remove(tx, &ix, old, id)?;
            }
        }
        if caps.undo == Undo::Log {
            tx.log(UndoEntry::Deleted { table: table.clone(), storage: Arc::clone(storage), id, row: old.clone() })?;
        }
        if Self::logs_changes(caps.persistence) {
            let change = Change::Delete { table: table.clone(), generation: self.generation_of(table), row: id, before: old.clone() };
            tx.log_sealed(change, self.seal_of(table)?.as_deref().map(|p| (p, id)))?;
        }
        Ok(())
    }
}

/// A row's change as a statement writes it.
enum Write {
    Update(Row),
    Delete,
}

/// What a statement does with a row another transaction changed after its
/// snapshot, given the version that transaction committed (RULES.md D29).
pub(crate) enum Again {
    /// Changes it to this row, computed from that version.
    Update(Row),
    /// Deletes it.
    Delete,
    /// Leaves it: its search condition no longer holds of it.
    Skip,
}

/// A statement's check of the version of a row another transaction
/// committed after its snapshot (`State::rewrite_row`).
pub(crate) type Recheck<'a> = &'a mut dyn FnMut(&Row) -> Result<Again>;

/// \[DESIGN\] How many times a row's write is refused by another
/// transaction's change before the statement is, bounding the waits for
/// an engine whose answers disagree.
const MOST_CONFLICTS: usize = 64;

impl Txn {
    /// The row it wrote of form `form`'s change to the kept totals of
    /// `table`, if it wrote one; a statement undone may have taken it.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the transaction's state is poisoned.
    pub(crate) fn kept_row(&self, table: &QualifiedName, form: usize) -> Result<Option<RowId>> {
        Ok(self.state()?.kept_rows.get(&(table.clone(), form)).copied())
    }

    /// Notes `row` as the one it writes of form `form`'s change to the kept
    /// totals of `table`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the transaction's state is poisoned.
    pub(crate) fn note_kept_row(&self, table: &QualifiedName, form: usize, row: RowId) -> Result<()> {
        self.state()?.kept_rows.insert((table.clone(), form), row);
        Ok(())
    }

    /// Notes whether row `row`'s entry in the unique index `index`, as it
    /// was inserted, was its key's only one.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the transaction's state is poisoned.
    pub(crate) fn note_alone(&self, index: &str, row: RowId, alone: bool) -> Result<()> {
        let mut state = self.state()?;
        if alone {
            state.alone.insert((index.to_owned(), row));
        } else {
            state.alone.remove(&(index.to_owned(), row));
        }
        Ok(())
    }

    /// Whether row `row`'s entry in the unique index `index`, as it was
    /// last inserted, was its key's only one: another row of its key,
    /// inserted after, was not, and is looked for itself.
    pub(crate) fn alone(&self, index: &str, row: RowId) -> bool {
        self.state().is_ok_and(|s| s.alone.contains(&(index.to_owned(), row)))
    }
}
