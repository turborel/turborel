//! \[DESIGN\] The rows of a stream (RULES.md D31) for an operator that
//! reads them whole: a sort (`exec/sort/vectors.rs`), and EXCEPT and
//! INTERSECT (`exec/set.rs`). A stream whose batches hold every column of
//! its rows, in order, gives its batches as they are, each with the rows a
//! filter kept; EXCEPT and INTERSECT are given each row's key too, made
//! straight from the vectors (`put_key`), as grouping makes it, and make a
//! row of the values only of a row they keep or write. Split from
//! batch.rs (RULES.md R-SIZE-2).

use std::sync::Arc;

use super::stream;
use crate::db::State;
use crate::error::Result;
use crate::physical::Plan;
use crate::spill::{put_norm, put_norm_approx, put_norm_exact, put_norm_text};
use crate::storage::{Batch, Cell, Row};
use crate::txn::TxId;
use crate::value::Value;

/// The width of the rows of `plan`, if it is a stream whose batches hold
/// every column of its rows, in order (`stream::layout`).
pub(in crate::exec) fn whole_width(plan: &Plan, db: &State) -> Option<usize> {
    stream::layout(plan, db).filter(|(columns, width)| columns.iter().copied().eq(0..*width)).map(|l| l.1)
}

/// Appends the key of `cell` to `out`: the bytes `spill::put_norm` gives of
/// its value, made without making it.
pub(in crate::exec) fn put_key(cell: Cell<'_>, out: &mut Vec<u8>) {
    match cell {
        Cell::Value(v) => put_norm(v, out),
        Cell::Exact(units, scale) => put_norm_exact(i128::from(units), scale, out),
        Cell::Approx(f) => put_norm_approx(f, out),
        Cell::Text(text) => put_norm_text(text, out),
        Cell::Null => put_norm(&Value::Null, out),
    }
}

/// A batch of a stream, the rows of it given, and the key of each.
pub(in crate::exec) struct Keyed {
    batch: Batch,
    /// The rows given, in order: those a filter kept, or all.
    rows: Vec<usize>,
    /// Each row's key, one after another.
    keys: Vec<u8>,
    /// Where each row's key ends in `keys`.
    ends: Vec<usize>,
}

/// The bytes a key of one column takes, room made for them at once: most
/// are a tag and 8 bytes (`spill::put_norm`).
const KEY_BYTES: usize = 9;

impl Keyed {
    /// The rows given of `batch`, `rows` or all, each keyed by every column.
    fn of(batch: Batch, rows: Option<Vec<usize>>) -> Keyed {
        let rows = rows.unwrap_or_else(|| (0..batch.rows).collect());
        let width = batch.columns.len();
        let (mut keys, mut ends) = (Vec::with_capacity(rows.len().saturating_mul(width).saturating_mul(KEY_BYTES)), Vec::new());
        for &r in &rows {
            for at in 0..width {
                put_key(batch.cell(at, r), &mut keys);
            }
            ends.push(keys.len());
        }
        Keyed { batch, rows, keys, ends }
    }

    /// How many rows it gives.
    pub(in crate::exec) fn len(&self) -> usize {
        self.rows.len()
    }

    /// The key of the `n`th row given.
    pub(in crate::exec) fn key(&self, n: usize) -> &[u8] {
        let start = n.checked_sub(1).and_then(|before| self.ends.get(before)).copied().unwrap_or(0);
        let end = self.ends.get(n).copied().unwrap_or(start);
        self.keys.get(start..end).unwrap_or_default()
    }

    /// The `n`th row given, made of its values.
    pub(in crate::exec) fn row(&self, n: usize) -> Row {
        let r = self.rows.get(n).copied().unwrap_or(usize::MAX);
        (0..self.batch.columns.len()).map(|at| self.batch.cell(at, r).to_value()).collect()
    }
}

/// A stream's batches, each keyed (`Keyed`).
pub(in crate::exec) type KeyedBatches = Box<dyn Iterator<Item = Result<Keyed>> + Send>;

/// The batches of `plan`, a stream whose batches hold every column of its
/// rows (`whole_width`), as `tx` sees them, each row keyed.
///
/// # Errors
///
/// As `stream::open`.
pub(in crate::exec) fn keyed(plan: Plan, db: &Arc<State>, tx: TxId) -> Result<KeyedBatches> {
    let stream = stream::open(plan, db, tx)?;
    Ok(Box::new(stream.batches.map(|kept| kept.map(|(batch, rows)| Keyed::of(batch, rows)))))
}

/// The batches of `plan`, a stream whose batches hold every column of its
/// rows (`whole_width`), as `tx` sees them, each with the rows a filter
/// kept, if one did.
///
/// # Errors
///
/// As `stream::open`.
pub(in crate::exec) fn kept(plan: Plan, db: &Arc<State>, tx: TxId) -> Result<stream::Batches> {
    Ok(stream::open(plan, db, tx)?.batches)
}
