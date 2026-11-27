//! \[DESIGN\] A load given by column into a table of indexes and keys
//! (RULES.md D1235). Its group is written and logged as its extent, as a
//! table of no index's is (D1148), then each index takes the group's
//! entries at once (`Index::insert_sorted`): a B-link tree in key order, a
//! leaf's run at a time; any other method each in turn, as `insert_from`
//! adds a row's. Each row whose key was not alone in a unique index is
//! noted for the statement's end, which checks it as a row inserted alone
//! is checked, waiting for another writer of its key. A foreign key's
//! distinct keys are each looked up before the group is written
//! (`RowsChecked::hold_by_column`). The checkpoint gate is held from the
//! group to its last entry, so a checkpoint holds both or neither, and
//! recovery mends the entries of a group logged after it (`recovery.rs`).

use std::sync::Arc;

use super::{Indexed, Together};
use crate::catalog::{ColumnDesc, ConstraintKind, TableDesc};
use crate::db::State;
use crate::error::{Code, Result, SqlError};
use crate::storage::lanes::Records;
use crate::storage::{Row, RowId};
use crate::txn::{Txn, UndoEntry};
use crate::value::{DataType, Value};

#[cfg(test)]
thread_local! {
    /// How many indexes take a group's entries before a test's statement
    /// fails, as a crash there would stop it: none, past its extent
    /// logged.
    pub(in crate::db) static FAIL_AFTER: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

/// Whether a load by column of `desc` writes groups: as a table that
/// qualifies for extents (RULES.md D1147), or of UNIQUE constraints of no
/// period and foreign keys too, whatever its indexes (D1235).
pub(in crate::db) fn keyed_qualifies(desc: &TableDesc) -> bool {
    let lob = |c: &ColumnDesc| matches!(c.data_type, DataType::Lob { .. });
    let own = |c: &crate::catalog::ConstraintDesc| match &c.kind {
        ConstraintKind::NotNull(_) | ConstraintKind::ForeignKey(_) | ConstraintKind::Unique { period: None, .. } => true,
        ConstraintKind::Check { reads, .. } => reads.is_empty(),
        ConstraintKind::Unique { .. } | ConstraintKind::Connection(_) => false,
    };
    desc.constraints.iter().all(own) && !desc.columns.iter().any(lob)
}

impl Together<'_> {
    /// \[DESIGN\] RULES.md D1235: takes its table, `desc`, as a load by
    /// column does: whether it writes groups of it.
    pub(in crate::db) fn by_column(&mut self, desc: &TableDesc) -> bool {
        self.qualifies = keyed_qualifies(desc);
        self.qualifies
    }

    /// The columns its indexes' keys read, in order, each once.
    pub(super) fn key_reads(&self) -> Vec<usize> {
        let mut reads: Vec<usize> = self.indexes.iter().flat_map(|ix| ix.desc.reads()).collect();
        reads.sort_unstable();
        reads.dedup();
        reads
    }
}

impl State {
    /// Adds the entries of `rows`, of row ids `ids`, to each index of
    /// `together`, and their undo entries: whether each row was alone
    /// under its key in every unique index.
    ///
    /// # Errors
    ///
    /// As `Index::insert_sorted` and logging.
    pub(super) fn index_group(tx: &Txn, together: &Together<'_>, (rows, ids): (&[Row], &[RowId])) -> Result<Vec<bool>> {
        let mut alone = vec![true; ids.len()];
        #[cfg(test)]
        failed_after()?;
        for ix in &together.indexes {
            index_sorted(tx, ix, (rows, ids), &mut alone)?;
            #[cfg(test)]
            failed_after()?;
        }
        Ok(alone)
    }
}

/// A test's failure, once as many indexes as it asked took their entries.
#[cfg(test)]
fn failed_after() -> Result<()> {
    match FAIL_AFTER.with(std::cell::Cell::get) {
        Some(0) => Err(SqlError::new(Code::Io, "[DESIGN] a test's failure past an index's entries")),
        left => {
            FAIL_AFTER.with(|f| f.set(left.map(|n| n.saturating_sub(1))));
            Ok(())
        }
    }
}

/// Adds the entries of `rows`, of ids `ids`, to the index of `ix`, as
/// `Index::insert_sorted` adds them, but of an index a load holds back,
/// and their undo entry, logged though they fail, as an entry absent is
/// removed as nothing: of a unique index, each row not alone marked so
/// in `alone`.
fn index_sorted(tx: &Txn, ix: &Indexed, (rows, ids): (&[Row], &[RowId]), alone: &mut [bool]) -> Result<()> {
    let mut entries = Vec::with_capacity(rows.len());
    let mut whose = Vec::with_capacity(rows.len());
    for (n, (row, &id)) in rows.iter().zip(ids).enumerate() {
        let key = ix.desc.computed_key(row)?;
        if ix.desc.holds(&key) {
            entries.push((key, State::entry_id(ix.partition, id)?));
            whose.push(n);
        }
    }
    let inserted = if ix.held { Ok(Vec::new()) } else { ix.index.insert_sorted(&entries, ix.desc.unique) };
    let lone = match inserted {
        Ok(lone) => lone,
        Err(e) => {
            tx.log(UndoEntry::IndexInsertedRows { table: ix.desc.table.clone(), index: Arc::clone(&ix.index), entries })?;
            return Err(e);
        }
    };
    if ix.desc.unique {
        for (&n, was) in whose.iter().zip(lone) {
            if let Some(slot) = alone.get_mut(n) {
                *slot = *slot && was;
            }
        }
    }
    if entries.is_empty() {
        return Ok(());
    }
    tx.log(UndoEntry::IndexInsertedRows { table: ix.desc.table.clone(), index: Arc::clone(&ix.index), entries })
}

/// The rows of `columns`, the columns of `desc` each by its records, of
/// the columns `read` alone decoded, the others null.
///
/// # Errors
///
/// One of its records does not decode.
pub(super) fn sparse_rows(desc: &TableDesc, columns: &[Records], read: &[usize]) -> Result<Vec<Row>> {
    let count = columns.first().map_or(0, Records::len);
    let mut rows = vec![vec![Value::Null; desc.columns.len()]; count];
    for &at in read {
        let (Some(records), Some(column)) = (columns.get(at), desc.columns.get(at)) else { continue };
        for (n, row) in rows.iter_mut().enumerate() {
            if let (Some(field), Some(slot)) = (records.record(n), row.get_mut(at)) {
                *slot = column.data_type.decode_record(field).ok_or_else(|| undecoded(desc))?;
            }
        }
    }
    Ok(rows)
}

/// Row `n` of `columns`, the columns of `desc` each by its records, each
/// value decoded.
///
/// # Errors
///
/// One of its records does not decode.
pub(in crate::db) fn decoded_row(desc: &TableDesc, columns: &[Records], n: usize) -> Result<Row> {
    let value = |(c, r): (&ColumnDesc, &Records)| {
        r.record(n).map_or(Ok(Value::Null), |f| c.data_type.decode_record(f).ok_or_else(|| undecoded(desc)))
    };
    desc.columns.iter().zip(columns).map(value).collect()
}

fn undecoded(desc: &TableDesc) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[DESIGN] a record of a load into {} does not decode", desc.name))
}

#[cfg(test)]
mod tests;
