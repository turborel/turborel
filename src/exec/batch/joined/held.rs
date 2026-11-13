//! \[DESIGN\] A join of streams' right operand held as vectors (RULES.md
//! D31, D38): its batches read, each with the rows a filter kept of it,
//! counted by the bytes their vectors hold; read whole within the work
//! area, joined into one batch (`Batch::joined`), each vector of the kind
//! its batches' were, and of each key, the places of its rows in that
//! batch, in order, a row of a null key in none, as none matches
//! [X3.135-1986 §5.11 GR2], but of a key on which a null matches a null
//! [ISO/IEC 9075-2:2023 §8.15] (`key_bucket`). So the right's columns are gathered into the
//! joined rows as the left's are, no value made. Past the work area, the
//! batches read and the rest of the operand are given back, made into rows
//! for the row path to build (`exec/join.rs`): of the operand's width, or
//! of the columns its batches hold alone.

use std::collections::HashMap;
use std::sync::Arc;

use super::super::stream::{self, Batches, Kept};
use super::{gathered, rows_of};
use crate::db::State;
use crate::error::Result;
use crate::exec::{RowIter, broken, execute};
use crate::physical::{JoinKey, Plan};
use crate::storage::{Batch, Cell};
use crate::txn::TxId;
use crate::value::{Bucket, Value, key_bucket, key_equal};

/// A right operand held as vectors: its rows, and of each key, the places
/// of its rows.
pub(super) struct HeldRight {
    pub(super) batch: Arc<Batch>,
    /// The place in `batch` of each key's column.
    pub(super) places: Vec<usize>,
    /// Of each key, whether a null matches a null.
    nulls: Vec<bool>,
    buckets: HashMap<Vec<Bucket>, Arc<[usize]>>,
}

/// A right operand read: held as vectors, or past the work area, its
/// batches.
pub(super) enum Read {
    Held(HeldRight),
    Rest(Rest),
}

/// The batches of a right operand past the work area: those read, then
/// the rest; the columns of its rows they hold, and its width.
pub(super) struct Rest {
    batches: Batches,
    columns: Vec<usize>,
    width: usize,
}

impl Rest {
    /// Its rows, each of the operand's width, its columns the batches do
    /// not hold null.
    pub(super) fn rows(self) -> RowIter {
        rows_of(self.batches, self.columns, self.width)
    }

    /// Its rows of the columns its batches hold alone, in their order.
    pub(super) fn narrow(self) -> RowIter {
        let width = self.columns.len();
        rows_of(self.batches, (0..width).collect(), width)
    }
}

impl HeldRight {
    /// The places of the rows of key `key`, in order.
    pub(super) fn rows_of(&self, key: &[Bucket]) -> &[usize] {
        self.buckets.get(key).map_or(&[], |rows| rows)
    }

    /// Whether the keys of row `p` match the left's `values`: equal
    /// [X3.135-1986 §5.11], integers of one scale by their units, else
    /// compared as values; or not distinct, of a key on which a null
    /// matches a null [ISO/IEC 9075-2:2023 §8.15].
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a value nests deeper than a type may.
    pub(super) fn keys_equal(&self, p: usize, values: &[Value]) -> Result<bool> {
        for ((&at, &nulls), v) in self.places.iter().zip(&self.nulls).zip(values) {
            if !equal(v, self.batch.cell(at, p), nulls)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// The places of the rows of key `key`, in order, shared.
    pub(super) fn shared_rows_of(&self, key: &[Bucket]) -> Arc<[usize]> {
        self.buckets.get(key).map_or_else(|| Arc::from([]), Arc::clone)
    }
}

/// The right operand `right`, keyed by the right columns of `keys`, read
/// and held while its vectors hold no more than `limit` bytes.
///
/// # Errors
///
/// The operand's; a key column its batches lack (SQLCODE -910).
pub(super) fn read(right: Plan, keys: &[JoinKey], limit: usize, (db, tx): (&Arc<State>, TxId)) -> Result<Read> {
    let (columns, width) = stream::layout(&right, db).unwrap_or_default();
    let places: Vec<usize> = keys
        .iter()
        .map(|k| columns.iter().position(|&c| c == k.right).ok_or_else(|| broken("a join key the right operand's batch lacks")))
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
            return Ok(Read::Rest(Rest { batches: Box::new(read.into_iter().chain(batches)), columns, width }));
        }
    }
    let batch = Batch::joined(parts);
    let nulls: Vec<bool> = keys.iter().map(|k| k.nulls).collect();
    let buckets = bucketed(&batch, &places, &nulls)?;
    Ok(Read::Held(HeldRight { batch: Arc::new(batch), places, nulls, buckets }))
}

/// Of each key of the rows of `batch`, its columns at `places`, the places
/// of its rows, in order: a row of a null key in none, as a null equals
/// nothing [X3.135-1986 §5.11 GR2], but of a key whose `nulls` says a null
/// matches a null [ISO/IEC 9075-2:2023 §8.15].
///
/// # Errors
///
/// SQLCODE -910 if a value nests deeper than a type may.
fn bucketed(batch: &Batch, places: &[usize], nulls: &[bool]) -> Result<HashMap<Vec<Bucket>, Arc<[usize]>>> {
    let mut buckets: HashMap<Vec<Bucket>, Vec<usize>> = HashMap::new();
    'rows: for r in 0..batch.rows {
        let mut key = Vec::with_capacity(places.len());
        for (&at, &matches_null) in places.iter().zip(nulls) {
            match key_bucket(&batch.cell(at, r).to_value(), matches_null)? {
                Some(b) => key.push(b),
                None => continue 'rows,
            }
        }
        buckets.entry(key).or_default().push(r);
    }
    Ok(buckets.into_iter().map(|(bucketed, rows)| (bucketed, Arc::from(rows))).collect())
}

/// Whether `v` matches the value of `cell` as a key's (`key_equal`): an
/// integer's units of one scale compared as they are, else the values.
///
/// # Errors
///
/// SQLCODE -910 if a value nests deeper than a type may.
fn equal(v: &Value, cell: Cell<'_>, nulls: bool) -> Result<bool> {
    if let (Value::Exact(d), Cell::Exact(units, scale)) = (v, cell)
        && d.scale == scale
    {
        return Ok(d.units == i128::from(units));
    }
    match cell {
        Cell::Value(w) => key_equal(v, w, nulls),
        Cell::Exact(..) | Cell::Approx(_) | Cell::Text(_) | Cell::Null => key_equal(v, &cell.to_value(), nulls),
    }
}
