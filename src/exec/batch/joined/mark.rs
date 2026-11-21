//! \[DESIGN\] A mark join of a stream (RULES.md D643): its right operand
//! read and held as the row path holds it (`exec/join/mark.rs`), once the
//! first left batch is read; each left batch given on whole, its selection
//! kept, with one more vector, of each row's mark. Of right rows held in
//! memory, the hashes of a batch's keys, and of its keys and marks, are
//! made a vector at a time, and probe their tables (RULES.md D603). Of each kept row only
//! the values the join's keys and conditions read are taken from the
//! batch, so no other value is copied.
//!
//! The join gives every left row on, so a runtime filter of a column of its
//! left that reaches it is told its left (`batch/runtime.rs`).

use super::super::late::Deferred;
use super::super::stream::{self, Batches, Kept};
use super::left_late_batches;
use super::pairs::kept_rows;
use crate::db::State;
use crate::error::Result;
use crate::exec::Db;
use crate::exec::join::{MarkSide, Marker};
use crate::exec::table::KeyHashes;
use crate::exec::work::Work;
use crate::exec::{RowIter, Runner, broken, execute};
use crate::logical::cond_columns;
use crate::physical::{JoinKey, MarkJoin, Plan};
use crate::storage::{Batch, Column, Row, Sel};
use crate::txn::TxId;
use crate::value::Value;

/// The columns of the left operand of `m` its keys and conditions read.
fn read(m: &MarkJoin) -> Vec<usize> {
    let mut pair = Vec::new();
    for c in m.cond.iter().chain(&m.compared) {
        cond_columns(c, &mut pair);
    }
    let mut read: Vec<usize> = m.keys.iter().chain(&m.marks).map(|k| k.left).collect();
    read.extend(pair.into_iter().filter(|&c| c < m.widths.0));
    read.sort_unstable();
    read.dedup();
    read
}

/// The columns of the mark join `m`'s rows its batches hold, its left
/// operand's then its mark, and their width, if its left is a stream whose
/// batches hold every column of it the join reads.
pub(super) fn layout(m: &MarkJoin, db: &State) -> Option<(Vec<usize>, usize)> {
    let (mut columns, width) = stream::layout(&m.left, db)?;
    if !read(m).iter().all(|c| columns.contains(c)) {
        return None;
    }
    columns.push(width);
    Some((columns, width.saturating_add(1)))
}

/// The batches of `plan`, a mark join of a stream.
///
/// # Errors
///
/// The right operand's; the spill area's; a column the left's batches lack.
pub(super) fn mark_batches(plan: Plan, db: &Db, tx: TxId) -> Result<Batches> {
    told_batches(plan, &Deferred::default(), (db, tx))
}

/// As `mark_batches`, the runtime filters `deferred` tells of its left's
/// columns handed to its left.
///
/// # Errors
///
/// As `mark_batches`.
pub(super) fn told_batches(plan: Plan, deferred: &Deferred, (db, tx): (&Db, TxId)) -> Result<Batches> {
    let Plan::MarkJoin(m) = plan else { return Err(broken("a mark join is not one")) };
    let (left_columns, width) = stream::layout(&m.left, db).ok_or_else(|| broken("a mark join's left operand gives no batches"))?;
    let places: Option<Vec<(usize, usize)>> =
        read(&m).into_iter().map(|c| left_columns.iter().position(|&held| held == c).map(|at| (at, c))).collect();
    let places = places.ok_or_else(|| broken("a column the mark join reads its left's batches lack"))?;
    let MarkJoin { left, right, keys, cond, marks, compared, .. } = *m;
    let hashed = Hashed::of((&keys, &marks), &left_columns)?;
    let marker = Marker { keys, cond, marks, compared, runner: Runner::new(db, tx) };
    let left = left_late_batches(left, (db, tx), &Deferred::default().with_told(deferred.filters().to_vec()))?;
    let right = Some(execute(right, db, tx));
    Ok(Box::new(MarkProbe { left, right, side: None, marker, places, width, hashed, work: Work::of(db, tx) }))
}

/// The places in a left batch of a mark join's keys, and of its keys and
/// marks, of each whether a null matches a null; and their hashes of the
/// batch it probes by.
struct Hashed {
    keys: (Vec<usize>, Vec<bool>),
    both: (Vec<usize>, Vec<bool>),
    of_keys: KeyHashes,
    of_both: KeyHashes,
}

impl Hashed {
    /// Of `keys` and `marks`, of a left whose batches hold `columns`.
    fn of((keys, marks): (&[JoinKey], &[JoinKey]), columns: &[usize]) -> Result<Hashed> {
        let placed = |on: &mut dyn Iterator<Item = &JoinKey>| -> Result<(Vec<usize>, Vec<bool>)> {
            let mut out = (Vec::new(), Vec::new());
            for k in on {
                out.0.push(columns.iter().position(|&c| c == k.left).ok_or_else(|| broken("a mark join's key its left's batches lack"))?);
                out.1.push(k.nulls);
            }
            Ok(out)
        };
        let keys_placed = placed(&mut keys.iter())?;
        let both = placed(&mut keys.iter().chain(marks))?;
        Ok(Hashed { keys: keys_placed, both, of_keys: KeyHashes::default(), of_both: KeyHashes::default() })
    }

    /// The hashes of the `n`th of the rows hashed (`Probe`).
    fn probe(&self, n: usize) -> (Option<u64>, Option<u64>) {
        let at = |h: &KeyHashes| h.hashes.get(n).copied().filter(|_| !h.misses.get(n).copied().unwrap_or(true));
        let keys = at(&self.of_keys);
        (keys, if self.both.0.len() > self.keys.0.len() { at(&self.of_both) } else { keys })
    }
}

/// A mark join's batches: each of its left operand's batches with marks.
struct MarkProbe {
    left: Batches,
    right: Option<RowIter>,
    side: Option<MarkSide>,
    marker: Marker,
    /// Of each column the join reads of a left row, its place in a left
    /// batch and its place in the row.
    places: Vec<(usize, usize)>,
    /// The left's degree.
    width: usize,
    hashed: Hashed,
    work: Work,
}

impl MarkProbe {
    /// `batch` with the marks of its rows `rows`, null at the others.
    fn marked(&mut self, mut batch: Batch, rows: &[usize]) -> Result<Batch> {
        if self.side.is_none() {
            let right = self.right.take().ok_or_else(|| broken("a mark join's right rows were read twice"))?;
            self.side = Some(MarkSide::of(right, &self.marker, &mut self.work)?);
        }
        let side = self.side.as_mut().ok_or_else(|| broken("a mark join holds no right rows"))?;
        let hashed = &mut self.hashed;
        if let Some(how) = side.hashing() {
            hashed.of_keys.of_batch((&batch, Some(rows)), (&hashed.keys.0, &hashed.keys.1), how)?;
            // Of no marks, the hashes of keys and marks are those of keys.
            if hashed.both.0.len() > hashed.keys.0.len() {
                hashed.of_both.of_batch((&batch, Some(rows)), (&hashed.both.0, &hashed.both.1), how)?;
            }
        }
        let in_memory = side.hashing().is_some();
        let mut marks = vec![Value::Null; batch.rows];
        let mut l: Row = vec![Value::Null; self.width];
        for (n, &r) in rows.iter().enumerate() {
            for &(at, c) in &self.places {
                let cell = l.get_mut(c).ok_or_else(|| broken("a mark join's left row is too narrow"))?;
                *cell = batch.cell(at, r).to_value();
            }
            let mark = marks.get_mut(r).ok_or_else(|| broken("a batch's row past its count"))?;
            let truth = if in_memory { side.mark_hashed(&l, hashed.probe(n), &self.marker)? } else { side.mark(&l, &self.marker)? };
            *mark = truth.value();
        }
        batch.columns.push(Column::Values(marks));
        Ok(batch)
    }
}

impl Iterator for MarkProbe {
    type Item = Result<Kept>;

    fn next(&mut self) -> Option<Self::Item> {
        let (batch, sel): (Batch, Option<Sel>) = match self.left.next()? {
            Ok(kept) => kept,
            Err(e) => return Some(Err(e)),
        };
        let rows = kept_rows(sel.clone(), batch.rows);
        Some(self.marked(batch, &rows).map(|b| (b, sel)))
    }
}
