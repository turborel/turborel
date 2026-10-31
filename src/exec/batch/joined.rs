//! \[DESIGN\] Joins of streams, a batch at a time (RULES.md D31): an inner
//! hash join, an outer join [ISO/IEC 9075:1992 §7.5] or a product
//! [X3.135-1986 §5.20 GR2b], of two subtrees that give batches, each column
//! the join reads of their rows to match them among those their batches
//! hold.
//!
//! An inner hash join, and a LEFT outer join on keys whose condition
//! beyond them cannot raise an error and whose operands have no condition
//! of their own, read the right operand first and build it as the row path
//! does (`exec/join.rs`), of its batches' rows if its tables read by batch;
//! held in the work area, each of the left operand's batches probes it, a
//! batch of the joined rows made of each, their columns the left's and the
//! right's after them, as a joined row is, the condition judged of each
//! pair of equal keys, of a row of the pair's values. Partitioned, an
//! inner join's rows are made row by row, as the row path makes them, and
//! gathered into batches. Of a LEFT join, a left row no right row matches
//! is joined with nulls; its right operand, outgrowing the work area,
//! moves to a table of the spill area, not partitions, so every left row
//! probes it in the left's order.
//!
//! Any other outer join (FULL, a RIGHT one not planned as the LEFT one of
//! its operands swapped (`physical/join.rs`), a LEFT one whose condition
//! can raise or whose operand has a condition of its own), and a product,
//! join the rows of their operands' batches, of a row each of the table's
//! width, as the row path joins its operands' rows (`exec/join/outer.rs`,
//! `exec/join/rescan.rs`), and gather the joined rows into batches: so
//! their operands are scanned, and filtered, by batch.

use std::collections::VecDeque;
use std::sync::Arc;

use super::stream::{self, Batches, Kept};
use crate::db::State;
use crate::error::Result;
use crate::exec::join::{Built, Full, Joining, Rescanned, Right, outer_of, partitioned_joined, product};
use crate::exec::work::Work;
use crate::exec::{RowIter, Runner, eval_cond, execute, failed};
use crate::logical::{Cond, OuterJoin, Truth, cond_columns};
use crate::physical::Plan;
use crate::storage::{BATCH_ROWS, Batch, Row, batches_of};
use crate::txn::TxId;
use crate::value::{Bucket, Value, bucket, compare};

/// The operands of `plan`, if it is a join: a product, a hash join or an
/// outer join.
pub(super) fn operands(plan: &Plan) -> Option<(&Plan, &Plan)> {
    match plan {
        Plan::Product { left, right } | Plan::HashJoin { left, right, .. } | Plan::OuterJoin { left, right, .. } => Some((left, right)),
        Plan::SeqScan { .. }
        | Plan::IndexScan { .. }
        | Plan::Filter { .. }
        | Plan::IndexJoin { .. }
        | Plan::Aggregate { .. }
        | Plan::Project { .. }
        | Plan::Append { .. }
        | Plan::SetOp { .. }
        | Plan::Values { .. }
        | Plan::System { .. }
        | Plan::Sort { .. }
        | Plan::Distinct { .. } => None,
    }
}

/// The columns of the join `plan`'s rows its batches hold, and the width
/// of its rows, if it is a join of streams (`stream::layout`) every column
/// of whose keys and conditions their batches hold.
pub(super) fn layout(plan: &Plan, db: &State) -> Option<(Vec<usize>, usize)> {
    let (left, right) = operands(plan)?;
    let (mut columns, left_width) = stream::layout(left, db)?;
    let (right_columns, right_width) = stream::layout(right, db)?;
    let held = |c: &usize| c.checked_sub(left_width).map_or_else(|| columns.contains(c), |r| right_columns.contains(&r));
    if !read(plan, left_width).iter().all(held) {
        return None;
    }
    columns.extend(right_columns.iter().map(|&c| c.saturating_add(left_width)));
    Some((columns, left_width.saturating_add(right_width)))
}

/// The columns of the pairs of its operands' rows the join `plan`, whose
/// left operand is `width` columns wide, reads to match them: of its
/// keys, its condition, and its operands' own conditions.
fn read(plan: &Plan, width: usize) -> Vec<usize> {
    let mut columns = Vec::new();
    let keys = match plan {
        Plan::HashJoin { keys, .. } => keys,
        Plan::OuterJoin { keys, cond, own, .. } => {
            let (left_own, right_own) = own.as_deref().map_or((None, None), |(l, r)| (l.as_ref(), r.as_ref()));
            for c in [cond.as_ref(), left_own].into_iter().flatten() {
                cond_columns(c, &mut columns);
            }
            let mut of_right = Vec::new();
            if let Some(c) = right_own {
                cond_columns(c, &mut of_right);
            }
            columns.extend(of_right.into_iter().map(|c| c.saturating_add(width)));
            keys
        }
        Plan::Product { .. }
        | Plan::SeqScan { .. }
        | Plan::IndexScan { .. }
        | Plan::Filter { .. }
        | Plan::IndexJoin { .. }
        | Plan::Aggregate { .. }
        | Plan::Project { .. }
        | Plan::Append { .. }
        | Plan::SetOp { .. }
        | Plan::Values { .. }
        | Plan::System { .. }
        | Plan::Sort { .. }
        | Plan::Distinct { .. } => return columns,
    };
    columns.extend(keys.iter().flat_map(|&(l, r)| [l, r.saturating_add(width)]));
    columns
}

/// The batches of the join `plan`, one of streams (`layout`).
///
/// # Errors
///
/// A table's storage missing; the right operand's errors.
pub(super) fn batches(plan: Plan, db: &Arc<State>, tx: TxId) -> Result<Batches> {
    match plan {
        Plan::Product { left, right } => multiplied((*left, *right), db, tx),
        Plan::HashJoin { left, right, keys, .. } => probed((*left, *right, keys), Kind::Inner, db, tx),
        Plan::OuterJoin { kind, left, right, keys, cond, own, widths, .. } => {
            if kind == OuterJoin::Left && own.is_none() && cond.as_ref().is_none_or(|c| !c.can_raise()) {
                return probed((*left, *right, keys), Kind::Left(cond, widths), db, tx);
            }
            Ok(rowwise(Joining { kind, keys, cond, own, widths }, (*left, *right), db, tx))
        }
        Plan::SeqScan { .. }
        | Plan::IndexScan { .. }
        | Plan::Filter { .. }
        | Plan::IndexJoin { .. }
        | Plan::Aggregate { .. }
        | Plan::Project { .. }
        | Plan::Append { .. }
        | Plan::SetOp { .. }
        | Plan::Values { .. }
        | Plan::System { .. }
        | Plan::Sort { .. }
        | Plan::Distinct { .. } => Err(crate::exec::broken("a join of streams is not one")),
    }
}

/// The rows of `plan`, a subtree that gives batches: made of them, each of
/// its width, its columns the batches do not hold null, if every table it
/// reads reads by batch; else read row by row, as a table that makes each
/// row of a batch gives them as fast so.
fn operand_rows(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    if !stream::reads_by_batch(&plan, db) {
        return execute(plan, db, tx);
    }
    let (columns, width) = stream::layout(&plan, db).unwrap_or_default();
    match stream::batches(plan, db, tx) {
        Ok(batches) => rows_of(batches, columns, width),
        Err(e) => failed(e),
    }
}

/// The rows of `batches`, those a filter kept of each, each of `width`
/// columns, the values of a batch's vectors at `columns`, the others null.
fn rows_of(batches: Batches, columns: Vec<usize>, width: usize) -> RowIter {
    Box::new(batches.flat_map(move |kept| match kept {
        Ok((batch, rows)) => {
            let made: Vec<Result<Row>> = rows
                .unwrap_or_else(|| (0..batch.rows).collect())
                .into_iter()
                .map(|r| {
                    let mut row = vec![Value::Null; width];
                    for (at, &c) in columns.iter().enumerate() {
                        if let Some(slot) = row.get_mut(c) {
                            *slot = batch.cell(at, r).to_value();
                        }
                    }
                    Ok(row)
                })
                .collect();
            made
        }
        Err(e) => vec![Err(e)],
    }))
}

/// The columns of a join's rows its batches hold, of its operands `left`
/// and `right`: the left's, then the right's after its width.
fn joined_columns(left: &Plan, right: &Plan, db: &State) -> Vec<usize> {
    let (mut columns, left_width) = stream::layout(left, db).unwrap_or_default();
    let (right_columns, _) = stream::layout(right, db).unwrap_or_default();
    columns.extend(right_columns.iter().map(|&c| c.saturating_add(left_width)));
    columns
}

/// `rows`, joined, gathered into batches of their values of `columns`.
fn gathered(rows: RowIter, columns: &[usize]) -> Batches {
    // Rows with no id, which a batch does not keep.
    Box::new(batches_of(Box::new(rows.map(|r| r.map(|row| (0, row)))), columns).map(|b| b.map(|b| (b, None))))
}

/// [X3.135-1986 §5.20 GR2b] The batches of the product of `left` and
/// `right`: their rows joined as the row path joins them
/// (`join::product`), the right's read first, and gathered into batches.
fn multiplied((left, right): (Plan, Plan), db: &Arc<State>, tx: TxId) -> Result<Batches> {
    let columns = joined_columns(&left, &right, db);
    let mut work = Work::of(db, tx);
    let (scanned, _) = Rescanned::of(operand_rows(right, db, tx), &mut work)?;
    Ok(gathered(product(operand_rows(left, db, tx), scanned), &columns))
}

/// [ISO/IEC 9075:1992 §7.5] The batches of the outer join `joining` says
/// of `left` and `right`: their rows joined as the row path joins them
/// (`join::outer_of`), the right's read first, and gathered into batches.
fn rowwise(joining: Joining, (left, right): (Plan, Plan), db: &Arc<State>, tx: TxId) -> Batches {
    let columns = joined_columns(&left, &right, db);
    let later = Arc::clone(db);
    let left_rows = move || operand_rows(left, &later, tx);
    gathered(outer_of(joining, operand_rows(right, db, tx), left_rows, (db, tx)), &columns)
}

/// Which rows a join of streams probing its built right operand gives.
enum Kind {
    /// The pairs that match.
    Inner,
    /// Those for which the condition, if any, is true, its operands of
    /// the widths given; and each left row none matches, with nulls.
    Left(Option<Cond>, (usize, usize)),
}

/// The batches of a hash join of `left` and `right` on `keys`, as `kind`
/// says.
fn probed((left, right, keys): (Plan, Plan, Vec<(usize, usize)>), kind: Kind, db: &Arc<State>, tx: TxId) -> Result<Batches> {
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
            let left = stream::batches(left, db, tx)?;
            let scratch = match &kind {
                Kind::Left(Some(_), (l, r)) => vec![Value::Null; l.saturating_add(*r)],
                Kind::Inner | Kind::Left(None, _) => Vec::new(),
            };
            let joined = Joined { left_columns, right_columns, kind, scratch, runner: Runner::new(db, tx) };
            Ok(Box::new(Probe { left, joined, built, ready: VecDeque::new(), _work: work }))
        }
        Right::Partitioned(parts) => {
            let mut columns = left_columns;
            columns.extend(right_columns.iter().map(|&c| c.saturating_add(left_width)));
            Ok(gathered(partitioned_joined(operand_rows(left, db, tx), parts, work), &columns))
        }
    }
}

/// How a hash join of streams makes its rows: the columns of its operands'
/// rows its batches hold, which rows it gives, and the row a condition is
/// judged of, of every column of the pair, with what runs its subqueries.
struct Joined {
    left_columns: Vec<usize>,
    /// The columns of the right operand's rows a joined batch holds.
    right_columns: Vec<usize>,
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
        if !equal {
            return Ok(false);
        }
        for (at, &c) in self.left_columns.iter().enumerate() {
            if let Some(slot) = self.scratch.get_mut(c) {
                *slot = batch.cell(at, r).to_value();
            }
        }
        for (slot, v) in self.scratch.iter_mut().skip(*width).zip(right) {
            slot.clone_from(v);
        }
        Ok(eval_cond(cond, &self.scratch, &self.runner)? == Truth::True)
    }
}

/// A hash join's batches: each of its left operand's probing its right,
/// built.
struct Probe {
    left: Batches,
    joined: Joined,
    built: Built,
    /// Batches of joined rows made, not yet given.
    ready: VecDeque<Batch>,
    /// The work area the right operand is built in, held while it is.
    _work: Work,
}

impl Probe {
    /// Probes the right operand with the rows of `batch` at `rows`, or
    /// all, each joined row's values put in batches of up to `BATCH_ROWS`.
    fn probe(&mut self, batch: &Batch, rows: Option<&[usize]>) -> Result<()> {
        let places: Option<Vec<(usize, usize)>> =
            self.built.keys().iter().map(|&(l, r)| self.joined.left_columns.iter().position(|&c| c == l).map(|at| (at, r))).collect();
        let places = places.ok_or_else(|| crate::exec::broken("a join key the left operand's batch lacks"))?;
        let left = self.joined.left_columns.len();
        let width = left.saturating_add(self.joined.right_columns.len());
        let mut out = Batch::empty(width);
        let all: Vec<usize>;
        let rows = if let Some(rows) = rows {
            rows
        } else {
            all = (0..batch.rows).collect();
            &all
        };
        let mut key: Vec<Bucket> = Vec::with_capacity(places.len());
        for &r in rows {
            let value = |at: usize| batch.value(at, r).unwrap_or(&Value::Null);
            key.clear();
            // A null equals nothing [X3.135-1986 §5.11 GR2].
            let keyed = places.iter().all(|&(at, _)| bucket(value(at)).map(|b| key.push(b)).is_some());
            let mut found = false;
            let equal = |right: &Row| {
                places.iter().all(|&(at, rc)| right.get(rc).and_then(|v| compare(value(at), v)) == Some(std::cmp::Ordering::Equal))
            };
            let candidates = if keyed { Some(self.built.bucket_rows(&key)?) } else { None };
            for right in candidates.into_iter().flatten() {
                let right = right?;
                if self.joined.keeps((batch, r), &right, equal(&right))? {
                    found = true;
                    put(&mut out, (batch, r, left), (Some(&right), &self.joined.right_columns));
                    full(&mut out, &mut self.ready, width);
                }
            }
            // [ISO/IEC 9075:1992 §7.5 GR4, GR5b] A LEFT join's left row
            // no right row matches, with nulls.
            if !found && matches!(self.joined.kind, Kind::Left(..)) {
                put(&mut out, (batch, r, left), (None, &self.joined.right_columns));
                full(&mut out, &mut self.ready, width);
            }
        }
        if out.rows > 0 {
            self.ready.push_back(out);
        }
        Ok(())
    }
}

/// Readies `out` in `ready` and starts another of `width` vectors, if it
/// holds `BATCH_ROWS`.
fn full(out: &mut Batch, ready: &mut VecDeque<Batch>, width: usize) {
    if out.rows >= BATCH_ROWS {
        ready.push_back(std::mem::replace(out, Batch::empty(width)));
    }
}

/// Puts in `out` a joined row: of `batch` the values of row `r` in its
/// first `left` vectors; then of `right` those of `right_columns`, or nulls.
fn put(out: &mut Batch, (batch, r, left): (&Batch, usize, usize), (right, right_columns): (Option<&Row>, &[usize])) {
    for (vector, at) in out.columns.iter_mut().zip(0..left) {
        vector.values_mut().push(batch.value(at, r).cloned().unwrap_or(Value::Null));
    }
    for (vector, &c) in out.columns.iter_mut().skip(left).zip(right_columns) {
        vector.values_mut().push(right.and_then(|row| row.get(c)).cloned().unwrap_or(Value::Null));
    }
    out.rows = out.rows.saturating_add(1);
}

impl Iterator for Probe {
    type Item = Result<Kept>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(batch) = self.ready.pop_front() {
                return Some(Ok((batch, None)));
            }
            let (batch, rows) = match self.left.next()? {
                Ok(kept) => kept,
                Err(e) => return Some(Err(e)),
            };
            if let Err(e) = self.probe(&batch, rows.as_deref()) {
                return Some(Err(e));
            }
        }
    }
}
