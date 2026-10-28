//! \[DESIGN\] Rows read a batch at a time (`TableStorage::scan_batches`,
//! RULES.md D31): each column asked for a vector of its values, all of one
//! length, so work over many rows needs no row made for each. Split from
//! storage.rs (RULES.md R-SIZE-2).

use super::{Row, Rows};
use crate::error::Result;
use crate::value::Value;

/// \[DESIGN\] The most rows of a batch: a vector, 2048.
pub const BATCH_ROWS: usize = 2048;

/// \[DESIGN\] Rows of a scan, by column: the values of each column asked
/// for, in the order asked, a vector of `rows` of them.
#[derive(Debug, Default)]
pub struct Batch {
    /// Each column's values.
    pub columns: Vec<Vec<Value>>,
    /// How many rows.
    pub rows: usize,
}

impl Batch {
    /// A batch of no rows of `columns` columns, room made for a whole one.
    pub fn empty(columns: usize) -> Batch {
        Batch { columns: (0..columns).map(|_| Vec::with_capacity(BATCH_ROWS)).collect(), rows: 0 }
    }

    /// Adds `row`'s values of `columns`, as a table's row holds them.
    pub fn push(&mut self, row: &Row, columns: &[usize]) {
        for (vector, &c) in self.columns.iter_mut().zip(columns) {
            vector.push(row.get(c).cloned().unwrap_or(Value::Null));
        }
        self.rows = self.rows.saturating_add(1);
    }
}

/// \[DESIGN\] Batches of a scan's rows.
pub type Batches = Box<dyn Iterator<Item = Result<Batch>> + Send>;

/// `rows`, by batch, of their values of `columns`.
pub fn batches_of(rows: Rows, columns: &[usize]) -> Batches {
    let columns = columns.to_vec();
    let mut rows = rows;
    Box::new(std::iter::from_fn(move || {
        let mut batch = Batch::empty(columns.len());
        while batch.rows < BATCH_ROWS {
            match rows.next() {
                Some(Ok((_, row))) => batch.push(&row, &columns),
                Some(Err(e)) => return Some(Err(e)),
                None => break,
            }
        }
        (batch.rows > 0).then_some(Ok(batch))
    }))
}
