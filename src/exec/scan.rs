//! The reads of a table's rows: a scan, in its storage's order or in an
//! order asked for, told the simple conditions a filter over it has, and a
//! probe of an index. A located read follows each row with its id
//! (`LogicalPlan::Scan`), so a MERGE can change the target row each joined
//! row was read of [ISO/IEC 9075-2:2023 §14.12 GR6a]. Each row is read only
//! while the statement is not cancelled (RULES.md D44).

use std::sync::Arc;

use super::entries::{Reading, entry_row};
use super::read::Asked;
use super::{RowIter, broken, execute, failed, sort};
use crate::catalog::{IndexDesc, KeyShape, QualifiedName};
use crate::db::State;
use crate::error::{Code, Result, SqlError};
use crate::index::{Entries as IndexEntries, KeyOrder, KeyProjection, Probe};
use crate::logical::{Cond, Most, SortKey, pushed_conjuncts};
use crate::physical::Plan;
use crate::storage::{self, Condition, Conjunction, Row, RowId, ScanOrder, TableStorage};
use crate::txn::{TxId, checked};
use crate::value::SortOrder;

/// A table to read: its name, the columns read, and whether each row is
/// followed by its id.
type ReadOf<'a> = (&'a QualifiedName, &'a [usize], bool);

/// The row `row` of id `id`, followed by the id if `located`: the last
/// column of a located read (`storage::located`).
pub(super) fn read_row(located: bool, (id, mut row): (RowId, Row)) -> Row {
    if located {
        row.push(storage::located(id));
    }
    row
}

/// The rows of the scan `plan`.
#[inline(never)]
pub(super) fn scanned_plan(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    let Plan::SeqScan { table, columns, order, backward, most, located, .. } = plan else { return failed(broken("a scan is not one")) };
    if order.is_empty() {
        return scanned((&table, &columns, located), None, db, tx);
    }
    ordered_scan((&table, &columns, located), (&order, backward, most), None, db, tx)
}

/// The rows of `table`, `columns` read, in the order of the columns
/// `order`, each descending if `backward`, as ORDER BY sorts them
/// [X3.135-1986 §8.3 GR3b], only those
/// `condition` may match if given: as its storage reads them so
/// (`TableStorage::scan_ordered`), else read and sorted, if it no longer
/// can, its option changed since the plan was made, the first `most` alone
/// if only those are asked for.
fn ordered_scan(
    (table, columns, located): ReadOf<'_>,
    (order, backward, most): ScanOrder<'_>,
    condition: Option<&Conjunction>,
    db: &Arc<State>,
    tx: TxId,
) -> RowIter {
    let storage = match db.storage_of(table) {
        Ok(storage) => Arc::clone(storage),
        Err(e) => return failed(e),
    };
    let stop = db.tx.stop_of(tx);
    if let Some(rows) = Arc::clone(&storage).scan_ordered(tx, columns, (order, backward, most), condition) {
        return Box::new(checked(rows.map(move |read| read.map(|r| read_row(located, r))), stop));
    }
    let read = match condition {
        Some(c) => storage.scan_where(tx, columns, c),
        None => storage.scan(tx, columns),
    };
    // Sorted as ORDER BY sorts, in a work area that spills (RULES.md D38).
    // A row's id, last, is no sort key.
    let keys: Vec<SortKey> = order.iter().map(|&column| SortKey::new(column, SortOrder::of(backward, None))).collect();
    let rows: RowIter = Box::new(checked(read.map(move |r| r.map(|r| read_row(located, r))), stop));
    match most {
        Some(rows_asked) => sort::first(&keys, rows, Most { rows: rows_asked, ties: false }, db, tx),
        None => sort::sort(&keys, rows, db, tx),
    }
}

/// The rows of `table`'s columns `columns`, only those `condition` may
/// match if given.
#[inline(never)]
fn scanned((table, columns, located): ReadOf<'_>, condition: Option<&Conjunction>, db: &Arc<State>, tx: TxId) -> RowIter {
    match db.storage_of(table) {
        Ok(storage) => {
            let storage = Arc::clone(storage);
            let read = match condition {
                Some(c) => storage.scan_where(tx, columns, c),
                None => storage.scan(tx, columns),
            };
            Box::new(checked(read.map(move |r| r.map(|r| read_row(located, r))), db.tx.stop_of(tx)))
        }
        Err(e) => failed(e),
    }
}

/// The rows of `input`, under a filter on `cond`: a scan is told the
/// conditions, if it has simple ones (`logical::pushed_conjuncts`).
pub(super) fn filtered_input(input: Plan, cond: &Cond, db: &Arc<State>, tx: TxId) -> RowIter {
    match (input, pushed_conjuncts(cond)) {
        (Plan::SeqScan { table, columns, order, backward, located, .. }, Some(condition)) => {
            if order.is_empty() {
                return scanned((&table, &columns, located), Some(&condition), db, tx);
            }
            ordered_scan((&table, &columns, located), (&order, backward, None), Some(&condition), db, tx)
        }
        (input, _) => execute(input, db, tx),
    }
}

/// The rows the index scan `plan` finds, in index order or reversed: of a
/// covering scan, made of their entries where it may (`entry_row`). Never
/// inlined into `execute`, whose frame every level of a plan adds to the
/// stack (RULES.md R-CF-6), as are the helpers below.
#[inline(never)]
pub(super) fn indexed(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    let Plan::IndexScan { table, index, probe, reverse, covering, columns: needed, located, .. } = plan else {
        return failed(broken("an index scan is not one"));
    };
    if let Probe::Xml(asked) = &probe
        && asked.answered
    {
        return super::answered::answered(db, tx, (&table, &index, asked), (&needed, located));
    }
    let width = covering.then(|| db.catalog.table(&table).map(|t| t.columns.len())).flatten();
    match probe_index(db, &table, &index, &probe, reverse) {
        Ok(Found { storage, ix, entries, projection, read }) => match read.record(&*storage, tx) {
            Ok(()) => {
                let mut reading = Reading::of(&ix, Some(&needed), projection);
                let rows = checked(entries, db.tx.stop_of(tx)).filter_map(move |entry| {
                    let row = |(key, id)| {
                        Ok(entry_row(&*storage, (&ix, width), &mut reading, (tx, id), key)?.map(|row| read_row(located, (id, row))))
                    };
                    entry.and_then(row).transpose()
                });
                Box::new(checked(rows, db.tx.stop_of(tx)))
            }
            Err(e) => failed(e),
        },
        Err(e) => failed(e),
    }
}

/// What an index found: its entries, and how to read their rows.
pub(super) struct Found {
    pub(super) storage: Arc<dyn TableStorage>,
    /// The index's key.
    pub(super) ix: KeyShape,
    /// The entries, in index order or reversed, read as they are taken.
    pub(super) entries: IndexEntries,
    /// Present only when entries contain projections of the original keys.
    pub(super) projection: Option<Arc<dyn KeyProjection>>,
    /// What the probe read, which callers record before taking an entry.
    pub(super) read: Asked,
}

/// What an engine is told a probe of `ix` reads, but for a region an index
/// names (`read.rs`). A key by a sort key is no
/// value of its columns, so the probe cannot say which rows it chose, and
/// the engine is told every row with a key: never narrower than the index,
/// so it may refuse more commits, and never fewer (RULES.md D60).
pub(in crate::exec) fn asked(ix: &KeyShape, probe: Probe) -> Condition {
    // D34, D106: storage cannot infer which rows a function or an XML
    // lookup selects. Record a
    // whole-table read, including nulls, so conflict checks miss no match.
    // RULES.md D92: nor which an expression's values select.
    if probe.opaque() || ix.has_expressions() {
        return Condition { columns: Vec::new(), probe: Probe::All };
    }
    let probe = if ix.is_collated() { Probe::All } else { probe };
    Condition { columns: ix.columns.clone(), probe }
}

/// What `index` finds for `probe`, in index order, or reversed, read as
/// it is taken: callers look for a cancel as each entry is read (RULES.md
/// D44). The rows are chosen by a condition: callers tell the table's
/// engine (`TableStorage::read_by_condition`).
pub(super) fn probe_index(db: &State, table: &QualifiedName, index: &str, probe: &Probe, reverse: bool) -> Result<Found> {
    let storage = Arc::clone(db.storage_of(table)?);
    let ix =
        db.catalog.index(index).map(IndexDesc::key_shape).ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] no index {index}")))?;
    let order = if reverse { KeyOrder::Descending } else { KeyOrder::Ascending };
    let index = db.index_of(index)?;
    let projection = index.key_projection();
    let read = Asked::of(&**index, &ix, probe);
    let entries = Arc::clone(index).search(probe, order);
    Ok(Found { storage, ix, entries, projection, read })
}

// Unit tests: located reads (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
