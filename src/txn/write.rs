//! The row writes of the data-changing statements [X3.135-1986 §8.4, §8.5,
//! §8.7, §8.11, §8.12]: each changes a row and its index entries together, and
//! logs what undoes it for the coordinator [X3.135-1986 §4.16, §7.3 GR3b i].
//! Split from txn.rs (RULES.md R-SIZE-4).

use std::sync::Arc;

use super::UndoEntry;
use crate::catalog::{IndexDesc, QualifiedName};
use crate::db::State;
use crate::error::{Code, Result, SqlError};
use crate::index::{Index, Key};
use crate::log::record::Change;
use crate::storage::{Row, RowId, project};
use crate::txn::{Concurrency, Txn, Undo};
use crate::value::Value;

impl State {
    fn table_indexes(&self, table: &QualifiedName) -> Vec<IndexDesc> {
        self.catalog.indexes(table).into_iter().cloned().collect()
    }

    /// The key of `row` in the index `ix`.
    fn index_key(ix: &IndexDesc, row: &Row) -> Result<Key> {
        project(row, &ix.columns)
            .map(Key)
            .ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] a row is shorter than index {}", ix.name)))
    }

    fn index_add(&self, tx: &Txn, ix: &IndexDesc, row: &Row, id: RowId) -> Result<()> {
        let key = Self::index_key(ix, row)?;
        if !key.0.iter().any(Value::is_null) {
            let index = self.index_handle(&ix.name)?;
            index.insert(key.clone(), id)?;
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

    /// [X3.135-1986 §8.7] Inserts `row`, with its index entries, under an
    /// exclusive lock, logging what undoes it [X3.135-1986 §4.16, §7.3 GR3b i].
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
    /// inserts.
    pub(crate) fn update_row(&self, tx: &Txn, table: &QualifiedName, id: RowId, row: &Row) -> Result<()> {
        self.prepare_write(tx, table)?;
        // After any wait for a lock: a checkpoint never waits on a waiter.
        let _gate = self.tx.gate.enter();
        let storage = self.storage_of(table)?;
        let old = storage.get(tx.id, id)?.ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] no row {id} in {table}")))?;
        storage.update(tx.id, id, row.clone())?;
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
                self.index_remove(tx, &ix, &old, id)?;
            }
            self.index_add(tx, &ix, row, id)?;
        }
        Ok(())
    }

    /// [X3.135-1986 §8.4, §8.5] Deletes row `id`, as `insert_row` inserts.
    pub(crate) fn delete_row(&self, tx: &Txn, table: &QualifiedName, id: RowId) -> Result<()> {
        self.prepare_write(tx, table)?;
        // After any wait for a lock: a checkpoint never waits on a waiter.
        let _gate = self.tx.gate.enter();
        let storage = self.storage_of(table)?;
        let old = storage.get(tx.id, id)?.ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] no row {id} in {table}")))?;
        storage.delete(tx.id, id)?;
        let caps = self.capabilities(table)?;
        // An engine keeping versions keeps the deleted version's entries
        // until it is removed, as for an update.
        if caps.concurrency == Concurrency::Coordinator {
            for ix in self.table_indexes(table) {
                self.index_remove(tx, &ix, &old, id)?;
            }
        }
        if caps.undo == Undo::Log {
            tx.log(UndoEntry::Deleted { table: table.clone(), storage: Arc::clone(storage), id, row: old.clone() })?;
        }
        if Self::logs_changes(caps.persistence) {
            let change = Change::Delete { table: table.clone(), generation: self.generation_of(table), row: id, before: old };
            tx.log_sealed(change, self.seal_of(table)?.as_deref().map(|p| (p, id)))?;
        }
        Ok(())
    }
}
