//! \[DESIGN\] An inner hash join of streams, and a LEFT outer join on keys
//! whose condition beyond them cannot raise an error and whose operands
//! have no condition of their own (RULES.md D31): the right operand read
//! first and built as the row path builds it (`exec/join.rs`), of its
//! batches' rows if its tables read by batch; held in the work area, each
//! of the left operand's batches probes it, the condition judged of each
//! pair of equal keys, of a row of the pair's values. Of a LEFT join, a
//! left row no right row matches is joined with nulls [ISO/IEC 9075:1992
//! §7.5 GR4, GR5b]; its right operand, outgrowing the work area, moves to a
//! table of the spill area, so every left row probes it in the left's
//! order. An inner join's right operand outgrowing it is partitioned
//! (`parted.rs`). Split from exec/batch/joined.rs (RULES.md R-SIZE-2).

use std::cmp::Ordering;
use std::sync::Arc;

use super::super::stream::{self, Batches, Kept};
use super::pairs::{Out, kept_rows};
use super::{gathered, in_order, left_batches, operand_rows, parted};
use crate::db::State;
use crate::error::Result;
use crate::exec::join::{Built, Full, Right, partitioned_joined};
use crate::exec::work::Work;
use crate::exec::{Runner, eval_cond};
use crate::logical::{Cond, Truth};
use crate::physical::Plan;
use crate::storage::{Batch, Row};
use crate::txn::TxId;
use crate::value::{Bucket, Value, bucket, compare};

/// Which rows a join of streams probing its built right operand gives.
pub(super) enum Kind {
    /// The pairs that match.
    Inner,
    /// Those for which the condition, if any, is true, its operands of
    /// the widths given; and each left row none matches, with nulls.
    Left(Option<Cond>, (usize, usize)),
}

/// The batches of a hash join of `left` and `right` on `keys`, as `kind`
/// says.
///
/// # Errors
///
/// The right operand's; the spill area's; a key the left's batches lack.
pub(super) fn probed((left, right, keys): (Plan, Plan, Vec<(usize, usize)>), kind: Kind, db: &Arc<State>, tx: TxId) -> Result<Batches> {
    let (left_columns, left_width) = stream::layout(&left, db).unwrap_or_default();
    let (right_columns, _) = stream::layout(&right, db).unwrap_or_default();
    let mut work = Work::of(db, tx);
    let full = match kind {
        Kind::Inner => Full::Partition,
        Kind::Left(..) => Full::Table,
    };
    // The right operand first, as the row path reads it.
    match Built::of(operand_rows(right, db, tx), keys, (&mut work, full))? {
        Right::Held(built) => {
            let places: Option<Vec<(usize, usize)>> =
                built.keys().iter().map(|&(l, r)| left_columns.iter().position(|&c| c == l).map(|at| (at, r))).collect();
            let places = places.ok_or_else(|| crate::exec::broken("a join key the left operand's batch lacks"))?;
            let scratch = match &kind {
                Kind::Left(Some(_), (l, r)) => vec![Value::Null; l.saturating_add(*r)],
                Kind::Inner | Kind::Left(None, _) => Vec::new(),
            };
            let out = Out::new(left_columns.len(), right_columns);
            let joined = Joined { left_columns, kind, scratch, runner: Runner::new(db, tx) };
            let left = left_batches(left, db, tx)?;
            Ok(Box::new(Probe { left, joined, built, places, out, _work: work }))
        }
        Right::Partitioned(parts) => {
            let mut columns = left_columns;
            // In no order but the partitions', where none is asked for.
            if !in_order(&left) {
                return parted::joined(operand_rows(left, db, tx), parts, work, (columns, right_columns));
            }
            columns.extend(right_columns.iter().map(|&c| c.saturating_add(left_width)));
            Ok(gathered(partitioned_joined(operand_rows(left, db, tx), parts, work), &columns))
        }
    }
}

/// Which pairs a hash join of streams keeps: the columns of its left
/// operand's rows its batches hold, which rows it gives, and the row a
/// condition is judged of, of every column of the pair, with what runs its
/// subqueries.
struct Joined {
    left_columns: Vec<usize>,
    kind: Kind,
    scratch: Row,
    runner: Runner,
}

impl Joined {
    /// Whether left row `r` of `batch` and `right` are a pair the join
    /// keeps: their keys equal, as `equal` says, and the condition, if
    /// any, true of them [ISO/IEC 9075:1992 §7.5 GR1c].
    fn keeps(&mut self, (batch, r): (&Batch, usize), right: &Row, equal: bool) -> Result<bool> {
        let Kind::Left(Some(cond), (width, _)) = &self.kind else { return Ok(equal) };
        Ok(equal && judged(cond, (&mut self.scratch, *width), (batch, r), (right, &self.left_columns), &self.runner)?)
    }
}

/// Whether `cond` is true of the pair of left row `r` of `batch`, whose
/// vectors hold `left_columns`, and `right`: of a row of their values made
/// in `scratch`, the left operand's `width` columns first.
fn judged(
    cond: &Cond,
    (scratch, width): (&mut Row, usize),
    (batch, r): (&Batch, usize),
    (right, left_columns): (&Row, &[usize]),
    runner: &Runner,
) -> Result<bool> {
    for (at, &c) in left_columns.iter().enumerate() {
        if let Some(slot) = scratch.get_mut(c) {
            *slot = batch.cell(at, r).to_value();
        }
    }
    for (slot, v) in scratch.iter_mut().skip(width).zip(right) {
        slot.clone_from(v);
    }
    Ok(eval_cond(cond, scratch, runner)? == Truth::True)
}

/// A hash join's batches: each of its left operand's probing its right,
/// built.
struct Probe {
    left: Batches,
    joined: Joined,
    built: Built,
    /// The place in a left batch of each key's left column, with its
    /// right column.
    places: Vec<(usize, usize)>,
    out: Out,
    /// The work area the right operand is built in, held while it is.
    _work: Work,
}

impl Probe {
    /// Probes the right operand with the rows of `batch` at `rows`.
    fn probe(&mut self, batch: &Batch, rows: &[usize]) -> Result<()> {
        let mut values: Vec<Value> = Vec::with_capacity(self.places.len());
        let mut key: Vec<Bucket> = Vec::with_capacity(self.places.len());
        for &r in rows {
            values.clear();
            values.extend(self.places.iter().map(|&(at, _)| batch.cell(at, r).to_value()));
            key.clear();
            // A null equals nothing [X3.135-1986 §5.11 GR2].
            let keyed = values.iter().all(|v| bucket(v).map(|b| key.push(b)).is_some());
            let mut found = false;
            let candidates = if keyed { Some(self.built.bucket_rows(&key)?) } else { None };
            for right in candidates.into_iter().flatten() {
                let right = right?;
                let equal =
                    self.places.iter().zip(&values).all(|(&(_, rc), v)| right.get(rc).and_then(|w| compare(v, w)) == Some(Ordering::Equal));
                if self.joined.keeps((batch, r), &right, equal)? {
                    found = true;
                    self.out.push(Some(batch), Some(r), Some(&right));
                }
            }
            // [ISO/IEC 9075:1992 §7.5 GR4, GR5b] A LEFT join's left row
            // no right row matches, with nulls.
            if !found && matches!(self.joined.kind, Kind::Left(..)) {
                self.out.push(Some(batch), Some(r), None);
            }
        }
        Ok(())
    }
}

impl Iterator for Probe {
    type Item = Result<Kept>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(given) = self.out.next() {
                return Some(given);
            }
            let (batch, rows) = match self.left.next()? {
                Ok(kept) => kept,
                Err(e) => return Some(Err(e)),
            };
            let rows = kept_rows(rows, batch.rows);
            let done = self.probe(&batch, &rows);
            self.out.settle(Some(&batch), done);
        }
    }
}
