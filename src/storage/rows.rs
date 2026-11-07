//! \[DESIGN\] Rows as a table gives them: a row, its id, and rows read as
//! they are pulled.

use crate::error::Result;
use crate::value::Value;

/// A row: one value per column, in column order.
pub type Row = Vec<Value>;

/// The values of `row` at the positions `columns`, in that order; `None` if a
/// position is past the end of the row.
pub fn project(row: &[Value], columns: &[usize]) -> Option<Row> {
    columns.iter().map(|&c| row.get(c).cloned()).collect()
}

/// Identifies a row within its table, for as long as the table exists.
pub type RowId = u64;

/// \[DESIGN\] What a table keeps from one read by row id to the next of one
/// scan (`TableStorage::get_near`), so rows side by side cost less to read.
/// Its fields are the built-in engine's; another leaves it as it is.
#[derive(Default)]
pub struct Near {
    /// The page and the tree's stamp of the last read (`tree/near.rs`).
    pub(crate) leaf: Option<(u64, u64)>,
    /// The reads in a row that its page did not answer.
    pub(crate) misses: u8,
    /// The value read, kept so each read does not allocate one.
    pub(crate) bytes: Vec<u8>,
}

/// \[DESIGN\] Rows a table gives, with their ids, read as they are pulled.
/// The iterator owns what it reads, so it can be kept between calls, and
/// moved to another thread with the cursor reading it.
pub type Rows = Box<dyn Iterator<Item = Result<(RowId, Row)>> + Send>;
