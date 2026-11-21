//! \[EXTENSION\] Rows inserted together (RULES.md D530): the rows of an
//! INSERT, of a query or of many rows of values, and of a bulk copy, put in
//! their table a batch at a time. The table's lock is taken once; each
//! batch's rows go to its storage, then to the log as one record
//! (`log/record/batch.rs`), then to each index, in the rows' order, so each
//! index is as inserting them one at a time leaves it: its entries, its
//! pages and the estimates read from them alike.
//!
//! Each row is undone as `insert_row`'s is, by its own undo entry and those
//! of its index entries, and redone from the record as an insert of its
//! own, so what a statement does, and what a crash keeps of it, is as if
//! its rows were inserted one at a time. A table whose rows that would not
//! be so for, a partition or a partitioned table, one whose rows are
//! sealed, or a row holding a large object, takes them one at a time.

use std::sync::Arc;

use super::super::integrity::Effects;
use crate::catalog::{IndexDesc, QualifiedName};
use crate::db::State;
use crate::error::{Code, Result, SqlError};
use crate::exec::HeldRows;
use crate::index::Index;
use crate::log::record::insert_rows_record;
use crate::spill::row_bytes;
use crate::storage::{Row, RowId};
use crate::txn::{Txn, Undo, UndoEntry};
use crate::value::Value;

/// \[DESIGN\] The rows a batch holds at most: enough that the costs of a
/// batch are shared by many rows, few enough that its rows are held in
/// memory twice at most.
const BATCH_ROWS: usize = 4096;
/// \[DESIGN\] The bytes of rows one record holds, about, at most: so a
/// record stays well within a log segment and a replay holds little.
const RECORD_BYTES: usize = 1 << 20;

impl State {
    /// Whether rows of `table` are inserted together: it is no partition
    /// and not partitioned, and its rows are not sealed.
    ///
    /// # Errors
    ///
    /// As `seal_of`.
    pub(crate) fn inserts_together(&self, table: &QualifiedName) -> Result<bool> {
        let p = self.catalog.partitioning();
        let whole = p.is_empty() || (!p.is_partitioned(table) && p.ancestors(table).is_empty());
        Ok(whole && self.seal_of(table)?.is_none())
    }

    /// [X3.135-1986 §8.7] Inserts `rows` into `table`, with their index
    /// entries, as `insert_row` inserts each: their row ids, in order.
    ///
    /// # Errors
    ///
    /// As `insert_row`'s.
    pub(crate) fn insert_rows(&self, tx: &Txn, table: &QualifiedName, rows: &[Row]) -> Result<Vec<RowId>> {
        for row in rows {
            crate::geometry::registered::check_row(&*self.catalog, table, row)?;
        }
        let one_at_a_time = rows.len() < 2 || rows.iter().flatten().any(Value::needs_keeping) || !self.inserts_together(table)?;
        if one_at_a_time {
            return rows.iter().map(|row| self.insert_row(tx, table, row)).collect();
        }
        self.prepare_write(tx, table)?;
        let storage = Arc::clone(self.storage_of(table)?);
        let caps = self.capabilities(table)?;
        let logs = Self::logs_changes(caps.persistence);
        // After any wait for a lock: a checkpoint never waits on a waiter.
        let _gate = self.tx.gate.enter();
        let mut ids = Vec::with_capacity(rows.len());
        let mut from = 0;
        while let Some(rest) = rows.get(from..).filter(|r| !r.is_empty()) {
            let taken = record_rows(rest);
            let part = rest.get(..taken).unwrap_or_default();
            let first = ids.len();
            for row in part {
                let id = storage.insert(tx.id, row.clone())?;
                ids.push(id);
                if caps.undo == Undo::Log {
                    tx.log(UndoEntry::Inserted { table: table.clone(), storage: Arc::clone(&storage), id })?;
                }
            }
            if logs {
                let made = ids.get(first..).unwrap_or_default();
                tx.log_encoded(
                    &insert_rows_record(tx.id, (table, self.generation_of(table)), made, part),
                    u64::try_from(made.len()).unwrap_or(u64::MAX),
                )?;
            }
            from = from.saturating_add(taken);
        }
        for (ix, partition) in self.table_indexes(table) {
            self.index_rows(tx, &ix, (rows, &ids), partition)?;
        }
        Ok(ids)
    }

    /// Adds the entries of `rows`, of row ids `ids`, to the index `ix`, in
    /// their order, as `index_add` adds each.
    fn index_rows(&self, tx: &Txn, ix: &IndexDesc, (rows, ids): (&[Row], &[RowId]), partition: Option<u64>) -> Result<()> {
        let index: Arc<dyn Index> = self
            .indexes
            .get(&ix.name)
            .map(Arc::clone)
            .ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] the index {} does not exist", ix.name)))?;
        for (row, &id) in rows.iter().zip(ids) {
            let key = ix.computed_key(row)?;
            if !ix.holds(&key) {
                continue;
            }
            let id = Self::entry_id(partition, id)?;
            // Of a unique index, whether the key's only entry, so its
            // constraint need not look for another row (`Txn::alone`).
            if ix.unique {
                let alone = index.insert_alone(key.clone(), id)?;
                tx.note_alone(&ix.name, id, alone)?;
            } else {
                index.insert(key.clone(), id)?;
            }
            tx.log(UndoEntry::IndexInserted { table: ix.table.clone(), index: Arc::clone(&index), key, id })?;
        }
        Ok(())
    }
}

/// How many of `rows`, at least one, one record holds: as many as come to
/// `RECORD_BYTES`.
fn record_rows(rows: &[Row]) -> usize {
    let mut bytes = 0_usize;
    let fit = rows.iter().take_while(|row| {
        bytes = bytes.saturating_add(row_bytes(row));
        bytes <= RECORD_BYTES
    });
    fit.count().max(1)
}

/// [X3.135-1986 §8.7 GR1c] Inserts the rows `rows` holds into `table`, of
/// transaction `tx`, noting each in `effects`: a batch at a time, if the
/// table takes its rows together; else one at a time, each where its
/// partition holds it (RULES.md D155).
///
/// # Errors
///
/// As `State::insert_rows`, `State::insert_routed` and `Effects::inserted`.
pub(super) fn insert_held(st: &State, tx: &Txn, table: &QualifiedName, rows: &mut HeldRows<Row>, effects: &mut Effects<'_>) -> Result<()> {
    if rows.len() < 2 || !st.inserts_together(table)? {
        return rows.visit(&mut |row| st.insert_routed(tx, table, row).and_then(|(leaf, id)| effects.inserted(&leaf, id, row)));
    }
    let mut batch: Vec<Row> = Vec::with_capacity(BATCH_ROWS.min(rows.len()));
    rows.visit(&mut |row| {
        batch.push(row.clone());
        if batch.len() < BATCH_ROWS { Ok(()) } else { inserted(st, (tx, table), &mut batch, effects) }
    })?;
    inserted(st, (tx, table), &mut batch, effects)
}

/// Inserts `batch` into `table` and notes each row in `effects`, leaving
/// it empty.
fn inserted(st: &State, (tx, table): (&Txn, &QualifiedName), batch: &mut Vec<Row>, effects: &mut Effects<'_>) -> Result<()> {
    if batch.is_empty() {
        return Ok(());
    }
    // Checked as each goes in where it may be, so most need no note.
    let mut checked = st.rows_checked(table, effects);
    if let Some(c) = &mut checked {
        for row in batch.iter() {
            c.check((st, tx), row)?;
        }
    }
    let ids = st.insert_rows(tx, table, batch)?;
    for (row, id) in batch.iter().zip(ids) {
        if checked.as_ref().is_none_or(|c| c.needs_note(tx, id)) {
            effects.inserted(table, id, row)?;
        }
    }
    batch.clear();
    Ok(())
}
