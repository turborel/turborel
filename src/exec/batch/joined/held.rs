//! \[DESIGN\] A join of streams' right operand held as vectors (RULES.md
//! D31, D38): its batches read, each with the rows a filter kept of it,
//! counted by the bytes their vectors hold; read whole within the work
//! area, joined into one batch (`Batch::joined`), each vector of the kind
//! its batches' were, and of each key, the places of its rows in that
//! batch, in order, a row of a null key in none, as none matches
//! [X3.135-1986 §5.11 GR2]. So the right's columns are gathered into the
//! joined rows as the left's are, no value made. Past the work area, the
//! rows read and the rest of the operand are given as rows, for the row
//! path to build (`exec/join.rs`). Split from exec/batch/joined.rs
//! (RULES.md R-SIZE-2).

use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;

use super::super::stream::{self, Batches, Kept};
use super::{gathered, rows_of};
use crate::db::State;
use crate::error::Result;
use crate::exec::{RowIter, broken, execute};
use crate::physical::Plan;
use crate::storage::{Batch, Cell};
use crate::txn::TxId;
use crate::value::{Bucket, Value, bucket, compare};

/// A right operand held as vectors: its rows, and of each key, the places
/// of its rows.
pub(super) struct HeldRight {
    pub(super) batch: Arc<Batch>,
    /// The place in `batch` of each key's column.
    pub(super) places: Vec<usize>,
    buckets: HashMap<Vec<Bucket>, Arc<[usize]>>,
}

/// A right operand read: held as vectors, or past the work area, its rows.
pub(super) enum Read {
    Held(HeldRight),
    Rows(RowIter),
}

impl HeldRight {
    /// The places of the rows of key `key`, in order.
    pub(super) fn rows_of(&self, key: &[Bucket]) -> &[usize] {
        self.buckets.get(key).map_or(&[], |rows| rows)
    }

    /// Whether the keys of row `p` equal the left's `values`
    /// [X3.135-1986 §5.11]: integers of one scale by their units, else
    /// compared as values.
    pub(super) fn keys_equal(&self, p: usize, values: &[Value]) -> bool {
        self.places.iter().zip(values).all(|(&at, v)| equal(v, self.batch.cell(at, p)))
    }

    /// The places of the rows of key `key`, in order, shared.
    pub(super) fn shared_rows_of(&self, key: &[Bucket]) -> Arc<[usize]> {
        self.buckets.get(key).map_or_else(|| Arc::from([]), Arc::clone)
    }
}

/// The right operand `right`, keyed by its columns `keys`, read and held
/// while its vectors hold no more than `limit` bytes.
///
/// # Errors
///
/// The operand's; a key column its batches lack (SQLCODE -910).
pub(super) fn read(right: Plan, keys: &[usize], limit: usize, (db, tx): (&Arc<State>, TxId)) -> Result<Read> {
    let (columns, width) = stream::layout(&right, db).unwrap_or_default();
    let places: Vec<usize> = keys
        .iter()
        .map(|k| columns.iter().position(|c| c == k).ok_or_else(|| broken("a join key the right operand's batch lacks")))
        .collect::<Result<_>>()?;
    let mut batches: Batches =
        if stream::reads_by_batch(&right, db) { stream::batches(right, db, tx)? } else { gathered(execute(right, db, tx), &columns) };
    let (mut parts, mut bytes) = (Vec::new(), 0_usize);
    for kept in batches.by_ref() {
        let (batch, rows) = kept?;
        let rows = rows.unwrap_or_else(|| (0..batch.rows).collect());
        bytes = bytes.saturating_add(batch.bytes()).saturating_add(rows.len().saturating_mul(size_of::<usize>()));
        parts.push((batch, rows));
        if bytes > limit {
            let read: Vec<Result<Kept>> = parts.into_iter().map(|(part, chosen)| Ok((part, Some(chosen)))).collect();
            return Ok(Read::Rows(rows_of(Box::new(read.into_iter().chain(batches)), columns, width)));
        }
    }
    let batch = Batch::joined(parts);
    let mut buckets: HashMap<Vec<Bucket>, Vec<usize>> = HashMap::new();
    let mut key = Vec::with_capacity(places.len());
    for r in 0..batch.rows {
        key.clear();
        // A null equals nothing [X3.135-1986 §5.11 GR2].
        let values: Vec<Value> = places.iter().map(|&at| batch.cell(at, r).to_value()).collect();
        if values.iter().all(|v| bucket(v).map(|b| key.push(b)).is_some()) {
            buckets.entry(key.clone()).or_default().push(r);
        }
    }
    let buckets = buckets.into_iter().map(|(bucketed, rows)| (bucketed, Arc::from(rows))).collect();
    Ok(Read::Held(HeldRight { batch: Arc::new(batch), places, buckets }))
}

/// Whether `v` equals the value of `cell` [X3.135-1986 §5.11]: an
/// integer's units of one scale compared as they are, else the values.
fn equal(v: &Value, cell: Cell<'_>) -> bool {
    if let (Value::Exact(d), Cell::Exact(units, scale)) = (v, cell)
        && d.scale == scale
    {
        return d.units == i128::from(units);
    }
    let order = match cell {
        Cell::Value(w) => compare(v, w),
        Cell::Exact(..) | Cell::Approx(_) | Cell::Text(_) | Cell::Null => compare(v, &cell.to_value()),
    };
    order == Some(Ordering::Equal)
}
