//! \[DESIGN\] Rows as a table gives them: a row, its id, and rows read as
//! they are pulled.

use std::any::Any;
use std::sync::Arc;

use super::Batch;
use crate::disk::buffer::Stamp;
use crate::error::{Code, Result, SqlError};
use crate::txn::{NO_TRANSACTION, Timestamp, TxId};
use crate::value::{Decimal, Value};

/// \[DESIGN\] What keeps and logs a table's large object's object
/// (`TableStorage::keep_lobs`): given its id, its body and what keeps it.
pub type LobLog<'a> = &'a mut dyn FnMut(u64, &[u8], &mut dyn FnMut() -> Result<()>) -> Result<()>;

/// A row: one value per column, in column order.
pub type Row = Vec<Value>;

/// The values of `row` at the positions `columns`, in that order; `None` if a
/// position is past the end of the row.
pub fn project(row: &[Value], columns: &[usize]) -> Option<Row> {
    columns.iter().map(|&c| row.get(c).cloned()).collect()
}

/// Appends to `out` the values of `row` at the positions `columns`, in
/// that order, NULL for one past its end, if there is a row: whether there
/// is, as the default `TableStorage::get_listed` gives a row read whole.
pub(super) fn listed(row: Option<Row>, columns: &[usize], out: &mut Row) -> bool {
    let Some(row) = row else { return false };
    out.extend(columns.iter().map(|&c| row.get(c).cloned().unwrap_or(Value::Null)));
    true
}

/// \[DESIGN\] Row `id` of `storage` as `tx` sees it, read whole, with each
/// column of `set` given its value in `row` (RULES.md D916): the row an
/// update that holds the values it sets alone writes, where it is written
/// whole.
pub(super) fn whole_set<S: super::TableStorage + ?Sized>(storage: &S, tx: TxId, id: RowId, (row, set): (&Row, &[usize])) -> Result<Row> {
    let mut whole = storage.get(tx, id)?.ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] no row {id}")))?;
    for &column in set {
        if let (Some(slot), Some(value)) = (whole.get_mut(column), row.get(column)) {
            slot.clone_from(value);
        }
    }
    Ok(whole)
}

/// The default `TableStorage::replace_set`: `replace`, of `row` as it is,
/// or, with a `patch`, of the row read whole and given the values `row`
/// holds of the columns set (`Patch::whole`).
pub(super) fn replace_whole<S: super::TableStorage + ?Sized>(
    storage: &S,
    (tx, id): (TxId, RowId),
    row: Option<&Row>,
    patch: Option<super::Patch<'_>>,
) -> Result<Row> {
    let (Some(new), Some(mut patch)) = (row, patch) else { return storage.replace(tx, id, row) };
    storage.replace(tx, id, Some(&patch.whole(storage, tx, id, new)?))
}

/// \[DESIGN\] The row of `values`, of `len` values at most, made once per
/// row read or computed: sized once, as a vector collected through
/// `Result` does not know its length and grows from a few values by
/// doubling, each growth a copy and an allocation.
///
/// # Errors
///
/// The first of `values`' errors.
pub fn sized_row(len: usize, values: impl Iterator<Item = Result<Value>>) -> Result<Row> {
    let mut row = Vec::with_capacity(len);
    for value in values {
        row.push(value?);
    }
    Ok(row)
}

/// Identifies a row within its table, for as long as the table exists.
pub type RowId = u64;

/// \[DESIGN\] The name of the column a located scan appends to each row
/// it reads: the row's id (`located`). No column of a table is named so,
/// a name of lower case letters no unquoted identifier is (`<regular
/// identifier>` [ISO/IEC 9075:1992 §5.2 SR10]), and no reference reaches
/// it, so it is read only by the statement that asked for it, a MERGE's.
pub const ROW_ID: &str = "row id";

/// \[DESIGN\] Row id `id` as the value a located scan gives it: an exact
/// number of scale 0, which holds every row id.
pub fn located(id: RowId) -> Value {
    Value::Exact(Decimal::new(i128::from(id), 0))
}

/// The row id `value` holds, a located scan's (`located`), or none if it
/// is null: of a row an outer join gave no row of the located table.
///
/// # Errors
///
/// SQLCODE -910 if `value` is no row id.
pub fn row_id_of(value: &Value) -> Result<Option<RowId>> {
    if value.is_null() {
        return Ok(None);
    }
    let not_one = || SqlError::new(Code::Api, "[DESIGN] a located scan's row id is an exact number of scale 0");
    let Value::Exact(d) = value else { return Err(not_one()) };
    if d.scale != 0 {
        return Err(not_one());
    }
    RowId::try_from(d.units).map(Some).map_err(|_| not_one())
}

/// \[DESIGN\] What a table keeps from one read by row id to the next of one
/// scan (`TableStorage::get_near`), so rows side by side cost less to read.
/// Its fields are the built-in engine's; another leaves it as it is.
#[derive(Default)]
pub struct Near {
    /// The page and the tree's stamp of the last read (`tree/near.rs`).
    pub(crate) leaf: Option<(u64, u64)>,
    /// The reads in a row that its page did not answer.
    pub(crate) misses: u8,
    /// The place in that page of the entry the last read found, where the
    /// next read looks first, just after it (`tree/near.rs`).
    pub(crate) at: Option<usize>,
    /// The value read, kept so each read does not allocate one.
    pub(crate) bytes: Vec<u8>,
    /// A copy of the page of the last read, taken once it answered two
    /// reads in a row, and the page and its frame's stamp then. Later
    /// reads of it take the copy while the stamp holds (`tree/near.rs`).
    pub(crate) page: Vec<u8>,
    pub(crate) kept: Option<(u64, Stamp)>,
    /// The reading transaction, the table and its bookkeeping there, found
    /// by the last read (`rows/mvcc/open.rs`).
    pub(crate) open: Option<(TxId, usize, Arc<dyn Any + Send + Sync>)>,
    /// \[DESIGN\] Of a read that found no row, the commit of the deletion
    /// its newest version is, if it is one: once no snapshot is before
    /// it, the row is gone for every reader, and so are its index entries
    /// (RULES.md D837). None of a row found, or of none in the tree.
    pub(crate) gone: Option<Timestamp>,
}

/// \[DESIGN\] Rows a table gives, with their ids, read as they are pulled.
/// The iterator owns what it reads, so it can be kept between calls, and
/// moved to another thread with the cursor reading it.
pub type Rows = Box<dyn Iterator<Item = Result<(RowId, Row)>> + Send>;

/// \[DESIGN\] Counts of a table's rows, which sum to how many there are
/// (`TableStorage::count`), read as they are pulled.
pub type Counts = Box<dyn Iterator<Item = Result<u64>> + Send>;

/// \[DESIGN\] (RULES.md D1181) A part of a table's rows as
/// `TableStorage::summarized` gives it: rows whole, of which only how many
/// there are is given and, of each column asked for, its nulls and its
/// least and greatest values that are not null, as the column's type
/// compares them; or a batch of rows of those columns.
pub enum Piece {
    /// Rows given whole.
    Whole {
        /// How many there are.
        rows: u64,
        /// Of each column asked for, its nulls and its least and greatest
        /// values, if any is not null.
        columns: Vec<(u64, Option<(Value, Value)>)>,
    },
    /// Rows given as a batch.
    Batch(Batch),
}

/// \[DESIGN\] The parts of a table's rows `TableStorage::summarized` gives,
/// read as they are pulled.
pub type Pieces = Box<dyn Iterator<Item = Result<Piece>> + Send>;

/// Makes row `id` of `table` hold `row`, or be absent, through `get`,
/// `update`, `restore` and `delete` outside any transaction, as the
/// default `TableStorage::apply` does.
///
/// # Errors
///
/// As those.
pub(super) fn applied<T: super::TableStorage + ?Sized>(table: &T, id: RowId, row: Option<Row>) -> Result<()> {
    match (row, table.get(NO_TRANSACTION, id)?.is_some()) {
        (Some(row), true) => table.update(NO_TRANSACTION, id, row),
        (Some(row), false) => table.restore(NO_TRANSACTION, id, row),
        (None, true) => table.delete(NO_TRANSACTION, id),
        (None, false) => Ok(()),
    }
}

/// \[DESIGN\] The order an ordered scan gives its rows in: the columns
/// sorted by, by their places, whether backward, and the most of its
/// first rows asked for, if FETCH FIRST asks only those
/// (`TableStorage::scan_ordered`).
pub type ScanOrder<'a> = (&'a [usize], bool, Option<usize>);

/// \[DESIGN\] The bytes a table takes (`TableStorage::sizes`, RULES.md
/// D522), counted, or, if `estimated`, of some rows their number times
/// their mean width; and what each of its columns takes, of each way its
/// values are kept (RULES.md D1165).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TableSizes {
    /// The bytes.
    pub bytes: u64,
    /// Whether some of them are estimated.
    pub estimated: bool,
    /// Of each column, by its place, and each way it is kept, what it takes.
    pub columns: Vec<ColumnSizes>,
}

/// \[DESIGN\] What a column takes, of one way its values are kept: the
/// chunks of one encoding of a column store's groups, or its values in the
/// rows of a tree (INFORMATION_SCHEMA.COLUMN_STORAGE, RULES.md D1165).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnSizes {
    /// The column's place.
    pub column: usize,
    /// The encoding of its chunks, as each chunk's first byte names it,
    /// `ROW` of its values in a tree's rows; none of a kind none names.
    pub encoding: Option<&'static str>,
    /// Its chunks; none of a tree's rows.
    pub chunks: Option<u64>,
    /// The rows they hold.
    pub rows: u64,
    /// Their bytes, encoded; none of a tree's rows, kept whole, or if
    /// not known.
    pub bytes: Option<u64>,
    /// The bytes of the pages its chunks take: a page each of a chunk on a
    /// page of its own, the chunk's bytes of one sharing its page; none of
    /// a tree's rows.
    pub page_bytes: Option<u64>,
}

// Unit tests: a row read whole given as its listed columns (RULES.md
// R-TEST-1).
#[cfg(test)]
mod tests {
    use super::{listed, located};
    use crate::value::Value;

    /// Verifies: [DESIGN] RULES.md D862, the default
    /// `TableStorage::get_listed`: of a row read whole, the values at the
    /// positions listed are appended in the order listed, one listed twice
    /// given twice and one past the row's end NULL; of no row, nothing is,
    /// and it says so.
    #[test]
    fn a_row_read_whole_gives_its_listed_columns_in_their_order() {
        let mut out = vec![located(9)];
        assert!(listed(Some(vec![located(0), located(1), located(2)]), &[2, 0, 2, 7], &mut out));
        let shown: Vec<String> = out.iter().map(ToString::to_string).collect();
        assert_eq!(shown, ["9", "2", "0", "2", &Value::Null.to_string()]);
        assert!(!listed(None, &[0], &mut out));
        assert_eq!(out.len(), 5);
    }
}
