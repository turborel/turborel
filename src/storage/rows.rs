//! \[DESIGN\] Rows as a table gives them: a row, its id, and rows read as
//! they are pulled.

use std::any::Any;
use std::sync::Arc;

use crate::error::{Code, Result, SqlError};
use crate::txn::TxId;
use crate::value::{Decimal, Value};

/// A row: one value per column, in column order.
pub type Row = Vec<Value>;

/// The values of `row` at the positions `columns`, in that order; `None` if a
/// position is past the end of the row.
pub fn project(row: &[Value], columns: &[usize]) -> Option<Row> {
    columns.iter().map(|&c| row.get(c).cloned()).collect()
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
    /// The reading transaction, the table and its bookkeeping there, found
    /// by the last read (`rows/mvcc/open.rs`).
    pub(crate) open: Option<(TxId, usize, Arc<dyn Any + Send + Sync>)>,
}

/// \[DESIGN\] Rows a table gives, with their ids, read as they are pulled.
/// The iterator owns what it reads, so it can be kept between calls, and
/// moved to another thread with the cursor reading it.
pub type Rows = Box<dyn Iterator<Item = Result<(RowId, Row)>> + Send>;

/// \[DESIGN\] The order an ordered scan gives its rows in: the columns
/// sorted by, by their places, whether backward, and the most of its
/// first rows asked for, if FETCH FIRST asks only those
/// (`TableStorage::scan_ordered`).
pub type ScanOrder<'a> = (&'a [usize], bool, Option<usize>);
