//! A row's insert and the index entries of a row written [X3.135-1986
//! §8.7]: each entry added or removed with what undoes it, and each row
//! change's record held for the log (RULES.md D25), through what its
//! statement resolved of the table once (`txn/writing.rs`, D872).

use std::borrow::Cow;
use std::sync::Arc;

use crate::catalog::{IndexDesc, QualifiedName};
use crate::db::State;
use crate::error::Result;
use crate::log::record::put_insert_record;
use crate::log::sealed::seal_plain;
use crate::storage::{Row, RowId};
use crate::txn::writing::{Entries, Writing};
use crate::txn::{Txn, Undo, UndoEntry};
use crate::value::Value;

impl State {
    /// The indexes `table`'s rows have entries in: its own, each entry of
    /// the row's id, and \[EXTENSION\] of a partition, the global indexes
    /// over it, each of the id packed with the partition's (RULES.md D157).
    /// Its own are borrowed from the catalog, not copied for each row
    /// written.
    pub(crate) fn table_indexes(&self, table: &QualifiedName) -> Vec<(Cow<'_, IndexDesc>, Option<u64>)> {
        let own = self.catalog.indexes(table).into_iter().map(|ix| (Cow::Borrowed(ix), None));
        own.chain(self.globals_over(table).into_iter().map(|(ix, partition)| (Cow::Owned(ix), Some(partition)))).collect()
    }

    /// The id of the entry of row `id` in an index, packed with `partition`.
    pub(crate) fn entry_id(partition: Option<u64>, id: RowId) -> Result<RowId> {
        partition.map_or(Ok(id), |p| crate::db::partitions::global::packed(p, id))
    }

    /// Adds the entry of `row`, as row `id`, to the index `ix`, as
    /// `index_add_in` does.
    pub(crate) fn index_add(&self, tx: &Txn, ix: &IndexDesc, row: &Row, id: RowId) -> Result<()> {
        Self::index_add_in(tx, &Entries::of(ix, self.indexes.get(&ix.name)), row, id)
    }

    /// Removes the entry of `row`, as row `id`, from the index `ix`, as
    /// `index_remove_in` does.
    pub(crate) fn index_remove(&self, tx: &Txn, ix: &IndexDesc, row: &Row, id: RowId) -> Result<()> {
        Self::index_remove_in(tx, &Entries::of(ix, self.indexes.get(&ix.name)), row, id)
    }

    /// Adds the entry of `row`, as row `id`, to the index of `entries`, if
    /// it holds the row's key, with what undoes it. An error an expression
    /// of the key raises fails the write (RULES.md D92).
    pub(crate) fn index_add_in(tx: &Txn, entries: &Entries<'_>, row: &Row, id: RowId) -> Result<()> {
        let ix = &entries.ix;
        let key = ix.computed_key(row)?;
        if ix.holds(&key) {
            let index = Arc::clone(entries.index()?);
            // Of a unique index, whether the key's only entry, so its
            // constraint need not look for another row (`Txn::alone`).
            if ix.unique {
                let alone = index.insert_alone(key.clone(), id)?;
                return tx.log_alone(&ix.name, (id, alone), UndoEntry::IndexInserted { table: ix.table.clone(), index, key, id });
            }
            index.insert(key.clone(), id)?;
            tx.log(UndoEntry::IndexInserted { table: ix.table.clone(), index, key, id })?;
        }
        Ok(())
    }

    /// Removes the entry of `row`, as row `id`, from the index of
    /// `entries`, as `index_add_in` adds one.
    pub(super) fn index_remove_in(tx: &Txn, entries: &Entries<'_>, row: &Row, id: RowId) -> Result<()> {
        let ix = &entries.ix;
        let key = ix.computed_key(row)?;
        if ix.holds(&key) {
            let index = Arc::clone(entries.index()?);
            index.remove(&key, id)?;
            tx.log(UndoEntry::IndexRemoved { table: ix.table.clone(), index, key, id })?;
        }
        Ok(())
    }

    /// Logs the record `record` makes, given the generation of the storage
    /// of the table of `writing`, of a change of its row `id`, sealed if
    /// its rows are; none made without a log. Made from the images as the
    /// statement holds them, none copied for it (RULES.md D750).
    pub(super) fn log_row(tx: &Txn, (writing, id): (&Writing<'_>, RowId), record: impl FnOnce(&mut Vec<u8>, u32)) -> Result<()> {
        if tx.origin.log.is_none() {
            return Ok(());
        }
        let generation = writing.generation;
        match &writing.seal {
            Some(pipeline) => {
                let mut plain = Vec::new();
                record(&mut plain, generation);
                tx.log_encoded(&seal_plain(&plain, id, pipeline)?, 1)
            }
            // Encoded where the transaction holds it (RULES.md D882).
            None => tx.log_written(1, |out| record(out, generation)),
        }
    }

    /// [X3.135-1986 §8.7] Inserts `row`, with its index entries, under its
    /// table's lock (`prepare_write`), logging what undoes it [X3.135-1986
    /// §4.16, §7.3 GR3b i].
    pub(crate) fn insert_row(&self, tx: &Txn, table: &QualifiedName, row: &Row) -> Result<RowId> {
        self.insert_row_in(tx, &self.writing(table)?, row)
    }

    /// As `insert_row`, into the table of `writing` (RULES.md D872).
    pub(crate) fn insert_row_in(&self, tx: &Txn, writing: &Writing<'_>, row: &Row) -> Result<RowId> {
        let table = writing.table();
        if writing.checked {
            crate::geometry::registered::check_row(&*self.catalog, table, row)?;
            // [EXTENSION] a partition holds only the rows its bounds hold (RULES.md D155).
            if !self.catalog.partitioning().is_empty() {
                self.check_holds(table, row)?;
            }
        }
        self.prepare_in(tx, writing)?;
        // A row of large objects is copied, so each names one the table
        // keeps for it, or is held in the row inside another value
        // (RULES.md D79, D106); any other is written as given.
        let kept = if row.iter().any(Value::needs_keeping) {
            let mut kept = row.clone();
            self.keep_lobs(tx, table, None, &mut kept)?;
            Cow::Owned(kept)
        } else {
            Cow::Borrowed(row)
        };
        let kept: &Row = &kept;
        // After any wait for a lock: a checkpoint never waits on a waiter.
        let _gate = self.tx.gate.enter();
        let id = writing.storage.insert_from(tx.id, kept)?;
        if writing.caps.undo == Undo::Log {
            tx.log(UndoEntry::Inserted { table: table.clone(), storage: Arc::clone(writing.storage), id })?;
        }
        if Self::logs_changes(writing.caps.persistence) {
            // Made from the row as it is held (RULES.md D531).
            Self::log_row(tx, (writing, id), |out, generation| put_insert_record(out, tx.id, (table, generation), id, kept))?;
        }
        for entries in &writing.indexes {
            Self::index_add_in(tx, entries, kept, Self::entry_id(entries.partition, id)?)?;
        }
        Ok(id)
    }
}
