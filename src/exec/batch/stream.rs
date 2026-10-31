//! \[DESIGN\] Batch streams (RULES.md D31): the rows of a subtree of a plan
//! given a batch at a time, for an aggregation or a projection over it to
//! read. A subtree gives them if it is a table's scan, one in an order
//! read in it and its rows gathered, as a plan over it relies on; a filter
//! of one, told the simple conjuncts of its condition (`scanned`) if it is
//! in none, or of any other stream; an inner hash join, a product or an
//! outer join of streams [ISO/IEC 9075:1992 §7.5] (`joined.rs`); a
//! projection of columns of a stream, its batches the vectors of those
//! columns, in order, moved; one of expressions of its columns, each
//! evaluated of the rows a batch keeps (`project::computed`); or the rows
//! of two streams each holding every column of their rows, alike
//! (`Plan::Append`, UNION ALL), a chain of them gathered without a frame
//! each, every operand's batches in turn, each opened once those before it
//! are read.
//!
//! A batch holds a vector of each column of the subtree's rows it reads,
//! in order of `Stream::columns`, and the rows of it a filter kept, if one
//! did.

use std::sync::Arc;

use super::joined;
use super::judge::Judging;
use super::project::{computed, place_of};
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::error::Result;
use crate::exec::Runner;
use crate::logical::{Cond, Expr, pushed_conjuncts};
use crate::physical::Plan;
use crate::storage::{Batch, Column};
use crate::txn::{TxId, checked};

/// A batch, and the rows of it a filter kept, if one did.
pub(in crate::exec) type Kept = (Batch, Option<Vec<usize>>);

/// What a stream gives, a batch at a time.
pub(in crate::exec) type Batches = Box<dyn Iterator<Item = Result<Kept>> + Send>;

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
        Plan::Product { .. } | Plan::HashJoin { .. } | Plan::OuterJoin { .. } => joined::layout(plan, db),
        Plan::Project { exprs, input, .. } => {
            let (columns, _) = layout(input, db)?;
            exprs.iter().all(|e| place_of(e, &columns).is_some()).then(|| ((0..exprs.len()).collect(), exprs.len()))
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
        | Plan::IndexJoin { .. }
        | Plan::Aggregate { .. }
        | Plan::Sort { .. }
        | Plan::Distinct { .. }
        | Plan::SetOp { .. }
        | Plan::Values { .. }
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
        Plan::Product { left, right } | Plan::HashJoin { left, right, .. } | Plan::OuterJoin { left, right, .. } => {
            reads_by_batch(left, db) && reads_by_batch(right, db)
        }
        Plan::IndexScan { .. }
        | Plan::IndexJoin { .. }
        | Plan::Aggregate { .. }
        | Plan::Sort { .. }
        | Plan::Distinct { .. }
        | Plan::SetOp { .. }
        | Plan::Values { .. }
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

/// The batches of `plan`, a subtree that gives them (`layout`).
///
/// # Errors
///
/// As `open`.
pub(super) fn batches(plan: Plan, db: &Arc<State>, tx: TxId) -> Result<Batches> {
    // A scan in an order (`TableStorage::scan_ordered`) is read in it, its
    // rows merged, and gathered into batches: a grouping or a join over it
    // gives its rows in that order, as the planner relies on.
    if let Plan::SeqScan { columns, order, .. } = &plan
        && !order.is_empty()
    {
        let columns = columns.clone();
        return Ok(joined::gathered(crate::exec::execute(plan, db, tx), &columns));
    }
    match plan {
        Plan::SeqScan { table, columns, .. } => scan_of((&table, &columns, None), db, tx),
        Plan::Filter { cond, input, .. } => {
            // A scan's filter: its simple conjuncts told the scan, unless
            // it is in an order.
            let inner = *input;
            let (inner, columns) = if let Plan::SeqScan { table, columns, order, .. } = &inner
                && order.is_empty()
            {
                (scan_of((table, columns, Some(&cond)), db, tx)?, columns.clone())
            } else {
                let columns = layout(&inner, db).map(|l| l.0).unwrap_or_default();
                (batches(inner, db, tx)?, columns)
            };
            Ok(filtered(inner, (Judging::of(&cond, &columns), Runner::new(db, tx))))
        }
        join @ (Plan::Product { .. } | Plan::HashJoin { .. } | Plan::OuterJoin { .. }) => joined::batches(join, db, tx),
        Plan::Project { exprs, input, .. } => {
            let columns = layout(&input, db).map(|l| l.0).unwrap_or_default();
            let inner = batches(*input, db, tx)?;
            if let Some(picks) = picked(&exprs, &columns) {
                return Ok(Box::new(inner.map(move |kept| kept.map(|(batch, rows)| (pick(batch, &picks), rows)))));
            }
            let places = exprs.iter().map(|e| place_of(e, &columns)).collect::<Option<Vec<_>>>();
            let places = places.ok_or_else(|| crate::exec::broken("a projection that gives no batches"))?;
            Ok(computed(inner, places, Runner::new(db, tx)))
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
        | Plan::IndexJoin { .. }
        | Plan::Aggregate { .. }
        | Plan::Sort { .. }
        | Plan::Distinct { .. }
        | Plan::SetOp { .. }
        | Plan::Values { .. }
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
/// keeps. A batch whose judgment raises an error gives first the rows a
/// filter read row by row would give before it (`Judging::in_order_of`),
/// then the error of the first row that raises one, so a reader that
/// stops at one of those rows, as EXISTS does, sees what it would row by
/// row (RULES.md D15).
fn filtered(inner: Batches, (judging, runner): (Judging, Runner)) -> Batches {
    Box::new(inner.flat_map(move |kept| {
        let judged = kept.map(|(batch, rows)| match judging.kept_of(&batch, rows.as_deref(), &runner) {
            Ok(judged) => vec![Ok((batch, Some(judged)))],
            Err(e) => {
                let (before, failed) = judging.in_order_of(&batch, rows.as_deref(), &runner);
                vec![Ok((batch, Some(before))), Err(failed.unwrap_or(e))]
            }
        });
        judged.unwrap_or_else(|e| vec![Err(e)])
    }))
}
