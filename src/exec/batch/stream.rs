//! \[DESIGN\] Batch streams (RULES.md D31): the rows of a subtree of a plan
//! given a batch at a time, for an aggregation or a projection over it to
//! read. A subtree gives them if it is a table's scan; a filter of one,
//! told the simple conjuncts of its condition (`scanned`), or of any other
//! stream; an inner hash join whose left operand is a stream; a LEFT
//! outer join of streams on keys, no other condition [ISO/IEC 9075:1992
//! §7.5]; a projection of columns of a stream, its batches the vectors of
//! those columns, in order, moved; or the rows of two streams each holding
//! every column of their rows, alike (`Plan::Append`, UNION ALL), a chain
//! of them gathered without a frame each, every operand's batches in turn,
//! each opened once those before it are read.
//!
//! A batch holds a vector of each column of the subtree's rows it reads,
//! in order of `Stream::columns`, and the rows of it a filter kept, if one
//! did. A hash join reads its right operand first, row by row, and builds
//! it as the row path does (`exec/join.rs`); held in the work area, its
//! left operand's batches each probe it, a batch of the joined rows made
//! of each, their columns the left's and the right's after them, as a
//! joined row is. Partitioned, the joined rows are made row by row, as the
//! row path makes them, and gathered into batches. Of a LEFT outer join,
//! a left row no right row matches is joined with nulls; its right
//! operand, outgrowing the work area, moves to a table of the spill area,
//! not partitions, so every left row probes it in the left's order.

use std::collections::VecDeque;
use std::sync::Arc;

use super::judge::Judging;
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::error::Result;
use crate::exec::join::{Built, Full, Right, partitioned_joined};
use crate::exec::work::Work;
use crate::exec::{Runner, execute};
use crate::logical::{Cond, Expr, OuterJoin, pushed_conjuncts};
use crate::physical::Plan;
use crate::storage::{BATCH_ROWS, Batch, Column, Row, batches_of};
use crate::txn::{TxId, checked};
use crate::value::{Bucket, Value, bucket, compare};

/// A batch, and the rows of it a filter kept, if one did.
pub(super) type Kept = (Batch, Option<Vec<usize>>);

/// What a stream gives, a batch at a time.
pub(super) type Batches = Box<dyn Iterator<Item = Result<Kept>> + Send>;

/// \[DESIGN\] A subtree's rows, a batch at a time.
pub(super) struct Stream {
    /// Its batches.
    pub(super) batches: Batches,
    /// The column of the subtree's rows each vector of a batch holds.
    pub(super) columns: Vec<usize>,
}

/// The columns of `plan`'s rows its batches hold, and the width of its
/// rows, if it is a subtree that gives batches.
pub(super) fn layout(plan: &Plan, db: &State) -> Option<(Vec<usize>, usize)> {
    match plan {
        Plan::SeqScan { table, columns, .. } => Some((columns.clone(), db.catalog.table(table)?.columns.len())),
        Plan::Filter { input, .. } => layout(input, db),
        Plan::HashJoin { left, right, .. } | Plan::OuterJoin { kind: OuterJoin::Left, left, right, cond: None, .. } => {
            let (mut columns, left_width) = layout(left, db)?;
            let (right_columns, right_width) = layout(right, db)?;
            columns.extend(right_columns.iter().map(|&c| c.saturating_add(left_width)));
            Some((columns, left_width.saturating_add(right_width)))
        }
        Plan::Project { exprs, input, .. } => {
            picked(exprs, &layout(input, db)?.0)?;
            Some(((0..exprs.len()).collect(), exprs.len()))
        }
        Plan::Append { .. } => {
            let mut held = None;
            for operand in appended(plan) {
                let made = layout(operand, db)?;
                // Each operand's batches hold every column of its rows, alike.
                if !made.0.iter().copied().eq(0..made.1) || held.as_ref().is_some_and(|h| h != &made) {
                    return None;
                }
                held = Some(made);
            }
            held
        }
        Plan::IndexScan { .. }
        | Plan::Product { .. }
        | Plan::OuterJoin { .. }
        | Plan::Aggregate { .. }
        | Plan::Sort { .. }
        | Plan::Distinct { .. }
        | Plan::SetOp { .. }
        | Plan::Values { .. }
        | Plan::DefaultRow { .. }
        | Plan::System { .. } => None,
    }
}

/// The operands of the chain of appends `plan`, in order, gathered on a
/// stack of their own: a chain is as long as its statement's UNIONs, and
/// a frame each would overflow the thread's (RULES.md R-CF-6).
fn appended(plan: &Plan) -> Vec<&Plan> {
    let (mut stack, mut out) = (vec![plan], Vec::new());
    while let Some(at) = stack.pop() {
        if let Plan::Append { left, right } = at {
            stack.push(right);
            stack.push(left);
        } else {
            out.push(at);
        }
    }
    out
}

/// `appended`, of a chain owned.
fn appended_owned(plan: Plan) -> Vec<Plan> {
    let (mut stack, mut out) = (vec![plan], Vec::new());
    while let Some(at) = stack.pop() {
        if let Plan::Append { left, right } = at {
            stack.push(*right);
            stack.push(*left);
        } else {
            out.push(at);
        }
    }
    out
}

/// The place among `columns` of each of `exprs`, if each is a column they
/// hold.
fn picked(exprs: &[Expr], columns: &[usize]) -> Option<Vec<usize>> {
    exprs.iter().map(|e| if let Expr::Column(c) = e { columns.iter().position(|held| held == c) } else { None }).collect()
}

/// Whether every table `plan` scans reads by batch
/// (`TableStorage::reads_by_batch`).
pub(super) fn reads_by_batch(plan: &Plan, db: &State) -> bool {
    match plan {
        // A scan in an order is read a row at a time, its rows merged.
        Plan::SeqScan { table, order, .. } => order.is_empty() && db.storage_of(table).is_ok_and(|s| s.reads_by_batch()),
        Plan::Filter { input, .. } | Plan::Project { input, .. } => reads_by_batch(input, db),
        Plan::Append { .. } => appended(plan).into_iter().all(|operand| reads_by_batch(operand, db)),
        Plan::HashJoin { left, right, .. } | Plan::OuterJoin { kind: OuterJoin::Left, left, right, cond: None, .. } => {
            reads_by_batch(left, db) && reads_by_batch(right, db)
        }
        Plan::IndexScan { .. }
        | Plan::Product { .. }
        | Plan::OuterJoin { .. }
        | Plan::Aggregate { .. }
        | Plan::Sort { .. }
        | Plan::Distinct { .. }
        | Plan::SetOp { .. }
        | Plan::Values { .. }
        | Plan::DefaultRow { .. }
        | Plan::System { .. } => false,
    }
}

/// The batches of `plan`, a subtree that gives them (`layout`), as `tx`
/// sees its rows.
///
/// # Errors
///
/// A table's storage missing; a hash join's right operand's errors.
pub(super) fn open(plan: Plan, db: &Arc<State>, tx: TxId) -> Result<Stream> {
    let (columns, _) = layout(&plan, db).ok_or_else(|| crate::exec::broken("a plan that gives no batches"))?;
    let batches = batches(plan, db, tx)?;
    Ok(Stream { batches, columns })
}

/// `open`'s batches.
fn batches(plan: Plan, db: &Arc<State>, tx: TxId) -> Result<Batches> {
    match plan {
        Plan::SeqScan { table, columns, .. } => scan_of((&table, &columns, None), db, tx),
        Plan::Filter { cond, input, .. } => {
            // A scan's filter: its simple conjuncts told the scan.
            let inner = *input;
            let (inner, columns) = if let Plan::SeqScan { table, columns, .. } = &inner {
                (scan_of((table, columns, Some(&cond)), db, tx)?, columns.clone())
            } else {
                let columns = layout(&inner, db).map(|l| l.0).unwrap_or_default();
                (batches(inner, db, tx)?, columns)
            };
            Ok(filtered(inner, (Judging::of(&cond, &columns), Runner::new(db, tx))))
        }
        Plan::HashJoin { left, right, keys, .. } => joined((*left, *right, keys), Kind::Inner, db, tx),
        Plan::OuterJoin { kind: OuterJoin::Left, left, right, keys, cond: None, .. } => joined((*left, *right, keys), Kind::Left, db, tx),
        Plan::Project { exprs, input, .. } => {
            let columns = layout(&input, db).map(|l| l.0).unwrap_or_default();
            let picks = picked(&exprs, &columns).ok_or_else(|| crate::exec::broken("a projection that gives no batches"))?;
            Ok(Box::new(batches(*input, db, tx)?.map(move |kept| kept.map(|(batch, rows)| (pick(batch, &picks), rows)))))
        }
        chain @ Plan::Append { .. } => {
            // Each operand opened once those before it are read.
            let later = Arc::clone(db);
            let operands = appended_owned(chain).into_iter();
            Ok(Box::new(
                operands.flat_map(move |operand| batches(operand, &later, tx).unwrap_or_else(|e| Box::new(std::iter::once(Err(e))))),
            ))
        }
        Plan::IndexScan { .. }
        | Plan::Product { .. }
        | Plan::OuterJoin { .. }
        | Plan::Aggregate { .. }
        | Plan::Sort { .. }
        | Plan::Distinct { .. }
        | Plan::SetOp { .. }
        | Plan::Values { .. }
        | Plan::DefaultRow { .. }
        | Plan::System { .. } => Err(crate::exec::broken("a plan that gives no batches")),
    }
}

/// `batch` with the vectors at `picks`, in order: each moved, or copied if
/// a later pick takes it too.
fn pick(batch: Batch, picks: &[usize]) -> Batch {
    let mut held: Vec<Option<Column>> = batch.columns.into_iter().map(Some).collect();
    let columns = picks
        .iter()
        .enumerate()
        .map(|(n, &at)| {
            let again = picks.get(n.saturating_add(1)..).is_some_and(|rest| rest.contains(&at));
            let vector = if again { held.get(at).cloned().flatten() } else { held.get_mut(at).and_then(Option::take) };
            vector.unwrap_or_default()
        })
        .collect();
    Batch { columns, rows: batch.rows }
}

/// A scan's batches (`scanned`), no row of them filtered yet.
fn scan_of(scan: (&QualifiedName, &[usize], Option<&Cond>), db: &Arc<State>, tx: TxId) -> Result<Batches> {
    Ok(Box::new(scanned(scan, db, tx)?.map(|b| b.map(|b| (b, None)))))
}

/// The batches of a scan of `table`'s `columns`, told the simple conjuncts
/// of `filter`, if any, as `tx` sees them, each read only while the
/// statement is not cancelled (RULES.md D44).
pub(super) fn scanned(
    (table, columns, filter): (&QualifiedName, &[usize], Option<&Cond>),
    db: &Arc<State>,
    tx: TxId,
) -> Result<impl Iterator<Item = Result<Batch>> + Send + use<>> {
    let storage = Arc::clone(db.storage_of(table)?);
    let pushed = filter.and_then(pushed_conjuncts);
    Ok(checked(storage.scan_batches(tx, columns, pushed.as_ref()), db.tx.stop_of(tx)))
}

/// `inner`'s batches, each with the rows of those kept that `judging`
/// keeps.
fn filtered(inner: Batches, (judging, runner): (Judging, Runner)) -> Batches {
    Box::new(inner.map(move |kept| {
        let (batch, rows) = kept?;
        let rows = judging.kept_of(&batch, rows.as_deref(), &runner)?;
        Ok((batch, Some(rows)))
    }))
}

/// Which rows a join of streams gives.
#[derive(Clone, Copy)]
enum Kind {
    /// The pairs that match.
    Inner,
    /// Those, and each left row no right row matches, with nulls.
    Left,
}

/// The batches of a hash join of `left` and `right` on `keys`, as `kind`
/// says.
fn joined((left, right, keys): (Plan, Plan, Vec<(usize, usize)>), kind: Kind, db: &Arc<State>, tx: TxId) -> Result<Batches> {
    let (left_columns, left_width) = layout(&left, db).unwrap_or_default();
    let (right_columns, _) = layout(&right, db).unwrap_or_default();
    let mut work = Work::new(db);
    let full = match kind {
        Kind::Inner => Full::Partition,
        Kind::Left => Full::Table,
    };
    // The right operand first, as the row path reads it.
    match Built::of(execute(right, db, tx), keys, (&mut work, full))? {
        Right::Held(built) => {
            let left = batches(left, db, tx)?;
            let probe = Probe { left, left_columns, right_columns, built, kind, ready: VecDeque::new(), _work: work };
            Ok(Box::new(probe))
        }
        Right::Partitioned(parts) => {
            let mut columns = left_columns;
            columns.extend(right_columns.iter().map(|&c| c.saturating_add(left_width)));
            // Rows with no id, which a batch does not keep.
            let rows = Box::new(partitioned_joined(execute(left, db, tx), parts, work).map(|r| r.map(|row| (0, row))));
            Ok(Box::new(batches_of(rows, &columns).map(|b| b.map(|b| (b, None)))))
        }
    }
}

/// A hash join's batches: each of its left operand's probing its right,
/// built.
struct Probe {
    left: Batches,
    left_columns: Vec<usize>,
    /// The columns of the right operand's rows a joined batch holds.
    right_columns: Vec<usize>,
    built: Built,
    kind: Kind,
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
            self.built.keys().iter().map(|&(l, r)| self.left_columns.iter().position(|&c| c == l).map(|at| (at, r))).collect();
        let places = places.ok_or_else(|| crate::exec::broken("a join key the left operand's batch lacks"))?;
        let width = self.left_columns.len().saturating_add(self.right_columns.len());
        let left = self.left_columns.len();
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
            let equal = |right: &&Row| {
                places.iter().all(|&(at, rc)| right.get(rc).and_then(|v| compare(value(at), v)) == Some(std::cmp::Ordering::Equal))
            };
            let candidates = if keyed { self.built.bucket_rows(&key)? } else { std::borrow::Cow::Borrowed(&[][..]) };
            for right in candidates.iter().filter(equal) {
                found = true;
                put(&mut out, (batch, r, left), (Some(right), &self.right_columns));
                full(&mut out, &mut self.ready, width);
            }
            // [ISO/IEC 9075:1992 §7.5 GR4, GR5b] A LEFT join's left row
            // no right row matches, with nulls.
            if !found && matches!(self.kind, Kind::Left) {
                put(&mut out, (batch, r, left), (None, &self.right_columns));
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
