//! \[DESIGN\] Batch streams (RULES.md D31): the rows of a subtree of a plan
//! given a batch at a time, for an aggregation or a projection over it to
//! read. A subtree gives them if it is a table's scan, one in an order
//! read in it and its rows gathered, as a plan over it relies on; a filter
//! of one, told the simple conjuncts of its condition (`scan_of`) if it is
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
use super::late::{Deferred, resolved};
use super::project::{computed, place_of};
use super::runtime::{self, Told};
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::error::Result;
use crate::exec::Runner;
use crate::logical::{Cond, Expr, pushed_conjuncts};
use crate::physical::Plan;
use crate::storage::{Batch, Column, Conjunct, Conjunction, Sel};
use crate::txn::{TxId, checked};

/// A batch, and the rows of it a filter kept, if one did.
pub(in crate::exec) type Kept = (Batch, Option<Sel>);

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
        Plan::SeqScan { table, columns, located: false, .. } => Some((columns.clone(), db.catalog.table(table)?.columns.len())),
        Plan::Filter { input, .. } => layout(input, db),
        // [EXTENSION] a counted operator's, its input's (RULES.md D378).
        Plan::Counted(c) => layout(&c.input, db),
        Plan::Product { .. } | Plan::HashJoin { .. } | Plan::OuterJoin { .. } | Plan::SemiJoin(_) | Plan::MarkJoin(_) => {
            joined::layout(plan, db)
        }
        Plan::Project { exprs, input, .. } => {
            let (columns, _) = layout(input, db)?;
            exprs.iter().all(|e| place_of(e, &columns).is_some()).then(|| ((0..exprs.len()).collect(), exprs.len()))
        }
        // A whole sort's batches hold every column of its rows, if its
        // input's do (RULES.md D380).
        Plan::Sort { input, most: None, .. } => {
            let (columns, width) = layout(input, db)?;
            columns.iter().copied().eq(0..width).then(|| ((0..width).collect(), width))
        }
        // A top-N's first rows, few, made batches (D380).
        Plan::Limit { input, .. } => matches!(**input, Plan::Sort { most: None, .. }).then(|| layout(input, db)).flatten(),
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
        // A located scan's rows end in their ids, which no batch holds.
        Plan::SeqScan { located: true, .. }
        | Plan::Nearest(_)
        | Plan::IndexScan { .. }
        | Plan::IndexJoin { .. }
        | Plan::MergeJoin(_)
        | Plan::RangeJoin(_)
        | Plan::Aggregate { .. }
        | Plan::Sort { .. }
        | Plan::MergeAppend { .. }
        | Plan::Distinct { .. }
        | Plan::SetOp { .. }
        | Plan::Values { .. }
        | Plan::System { .. }
        | Plan::Recursive { .. }
        | Plan::Unnest(..)
        | Plan::Window { .. }
        | Plan::Polymorphic(..)
        | Plan::Recognize { .. }
        | Plan::WorkTable { .. }
        | Plan::Transition { .. } => None,
        // A pipeline run in parallel, by batch (`parallel/batches.rs`).
        Plan::Parallel(p) => crate::exec::parallel::pipelined(&p.input).then(|| layout(&p.input, db)).flatten(),
    }
}

/// The operands of the chain of appends `plan`, in order, gathered on a
/// stack of their own: a chain is as long as its statement's UNIONs, and
/// a frame each would overflow the thread's (RULES.md R-CF-6).
fn appended(plan: &Plan) -> Vec<&Plan> {
    let (mut stack, mut out) = (vec![plan], Vec::new());
    while let Some(at) = stack.pop() {
        if let Plan::Append { left, right, .. } = at {
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
        if let Plan::Append { left, right, .. } = at {
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
pub(super) fn picked(exprs: &[Expr], columns: &[usize]) -> Option<Vec<usize>> {
    exprs.iter().map(|e| if let Expr::Column(c) = e { columns.iter().position(|held| held == c) } else { None }).collect()
}

/// Whether every table `plan` scans reads by batch
/// (`TableStorage::reads_by_batch`).
pub(super) fn reads_by_batch(plan: &Plan, db: &State) -> bool {
    match plan {
        // A scan in an order too, merged by batch
        // (`TableStorage::scan_ordered_batches`).
        Plan::SeqScan { table, located, .. } => !located && db.storage_of(table).is_ok_and(|s| s.reads_by_batch()),
        // A whole sort's rows are held, so given by batch whatever its
        // input's tables (RULES.md D380).
        Plan::Filter { input, .. } | Plan::Project { input, .. } | Plan::Sort { input, most: None, .. } => reads_by_batch(input, db),
        Plan::Limit { input, .. } => matches!(**input, Plan::Sort { most: None, .. }) && reads_by_batch(input, db),
        Plan::Counted(c) => reads_by_batch(&c.input, db),
        Plan::Append { .. } => appended(plan).into_iter().all(|operand| reads_by_batch(operand, db)),
        Plan::Product { left, right, .. } | Plan::HashJoin { left, right, .. } | Plan::OuterJoin { left, right, .. } => {
            reads_by_batch(left, db) && reads_by_batch(right, db)
        }
        Plan::SemiJoin(s) => reads_by_batch(&s.left, db) && reads_by_batch(&s.right, db),
        // Its right's rows are held as the row path holds them.
        Plan::MarkJoin(m) => reads_by_batch(&m.left, db),
        Plan::Nearest(_)
        | Plan::IndexScan { .. }
        | Plan::IndexJoin { .. }
        | Plan::MergeJoin(_)
        | Plan::RangeJoin(_)
        | Plan::Aggregate { .. }
        | Plan::Sort { .. }
        | Plan::MergeAppend { .. }
        | Plan::Distinct { .. }
        | Plan::SetOp { .. }
        | Plan::Values { .. }
        | Plan::System { .. }
        | Plan::Recursive { .. }
        | Plan::Unnest(..)
        | Plan::Window { .. }
        | Plan::Polymorphic(..)
        | Plan::Recognize { .. }
        | Plan::WorkTable { .. }
        | Plan::Transition { .. } => false,
        Plan::Parallel(p) => crate::exec::parallel::pipelined(&p.input) && reads_by_batch(&p.input, db),
    }
}

/// The batches of `plan`, a scan in an order (`TableStorage::scan_ordered`),
/// read in it: by batch where its table merges so, else its rows merged and
/// gathered. A grouping or a join over it gives its rows in that order, as
/// the planner relies on.
///
/// # Errors
///
/// As `batches`.
fn ordered_batches(plan: Plan, db: &Arc<State>, tx: TxId) -> Result<Batches> {
    if let Plan::SeqScan { table, columns, order, backward, most, .. } = &plan {
        let storage = Arc::clone(db.storage_of(table)?);
        if let Some(batches) = storage.scan_ordered_batches(tx, columns, (order, *backward, *most), None) {
            return Ok(Box::new(checked(batches, db.tx.stop_of(tx)).map(|b| b.map(|b| (b, None)))));
        }
    }
    let columns = layout(&plan, db).map(|l| l.0).unwrap_or_default();
    Ok(joined::gathered(crate::exec::execute(plan, db, tx), &columns))
}

/// The batches of `plan`, a subtree that gives them (`layout`), as `tx`
/// sees its rows.
///
/// # Errors
///
/// A table's storage missing; a hash join's right operand's errors.
pub(super) fn open(plan: Plan, db: &Arc<State>, tx: TxId) -> Result<Stream> {
    let (columns, _) = layout(&plan, db).ok_or_else(|| crate::exec::broken("a plan that gives no batches"))?;
    let batches = whole_batches(plan, db, tx)?;
    Ok(Stream { batches, columns })
}

/// \[DESIGN\] (RULES.md D361) The batches of `plan`, a subtree that gives
/// them (`layout`), as the stream is read whole: its scans' columns no
/// operator of it reads given late where one drops rows (`late.rs`), and
/// decoded as each batch is given, of the rows kept alone.
///
/// # Errors
///
/// As `open`.
pub(super) fn whole_batches(plan: Plan, db: &Arc<State>, tx: TxId) -> Result<Batches> {
    let width = layout(&plan, db).map_or(0, |(_, width)| width);
    Ok(resolved(batches_with(plan, (db, tx), &Deferred::all(width))?))
}

/// A function giving the batches of a plan of one kind, its columns the
/// operators above leave unread as the `Deferred` says.
type Batched = fn(Plan, (&Arc<State>, TxId), &Deferred) -> Result<Batches>;

/// The batches of `plan`, a subtree that gives them (`layout`). Each kind
/// of plan is made by a function of its own, chosen by reference and
/// handed the plan at one call, so this one's frame, which each level of
/// a plan adds to the stack, holds none of their locals and no copy of a
/// plan for each (RULES.md R-CF-6).
///
/// # Errors
///
/// As `open`.
pub(super) fn batches(plan: Plan, db: &Arc<State>, tx: TxId) -> Result<Batches> {
    batches_with(plan, (db, tx), &Deferred::default())
}

/// As `batches`, its scans' columns `deferred` leaves unread given late
/// where it says an operator above drops rows (RULES.md D361).
///
/// # Errors
///
/// As `open`.
pub(super) fn batches_with(plan: Plan, (db, tx): (&Arc<State>, TxId), deferred: &Deferred) -> Result<Batches> {
    if !deferred.filters().is_empty() {
        let (kept, here) = runtime::parted(&plan, deferred.filters(), db);
        if !here.is_empty() {
            return judged_above(plan, (db, tx), &deferred.clone().with_told(kept), here);
        }
    }
    let made: Batched = match &plan {
        Plan::SeqScan { .. } => scan_batches,
        Plan::Filter { .. } => filter_batches,
        Plan::Product { .. } | Plan::HashJoin { .. } | Plan::OuterJoin { .. } | Plan::SemiJoin(_) | Plan::MarkJoin(_) => {
            joined::batches_with
        }
        Plan::Project { .. } => projected_batches,
        Plan::Append { .. } => appended_batches,
        Plan::Counted(_) => super::counted::batches,
        Plan::Nearest(_)
        | Plan::IndexScan { .. }
        | Plan::IndexJoin { .. }
        | Plan::MergeJoin(_)
        | Plan::RangeJoin(_)
        | Plan::Aggregate { .. }
        | Plan::MergeAppend { .. }
        | Plan::Distinct { .. }
        | Plan::SetOp { .. }
        | Plan::Values { .. }
        | Plan::System { .. }
        | Plan::Recursive { .. }
        | Plan::Unnest(..)
        | Plan::Window { .. }
        | Plan::Polymorphic(..)
        | Plan::Recognize { .. }
        | Plan::WorkTable { .. }
        | Plan::Transition { .. } => none,
        Plan::Parallel(..) => parallel_batches,
        Plan::Sort { .. } => sorted_batches,
        Plan::Limit { .. } => topped_batches,
    };
    made(plan, (db, tx), deferred)
}

/// The batches of `plan`, a pipeline run in parallel, in order
/// (`parallel/batches.rs`); of one that gives way, those of its plan
/// alone.
///
/// # Errors
///
/// As `open`.
fn parallel_batches(plan: Plan, (db, tx): (&Arc<State>, TxId), deferred: &Deferred) -> Result<Batches> {
    let Plan::Parallel(p) = plan else { return Err(crate::exec::broken("a part run in parallel is not one")) };
    // A part whose batches hold no layout of its own is read as its plan.
    let Some(shape) = layout(&p.input, db) else { return batches_with(p.input, (db, tx), deferred) };
    match crate::exec::parallel::streamed_batches(&p.input, (db, tx), p.degree, &shape)? {
        crate::exec::parallel::Held::Kept(batches) => Ok(batches),
        crate::exec::parallel::Held::GaveWay => batches_with(p.input, (db, tx), deferred),
    }
}

/// \[DESIGN\] (RULES.md D380) The batches of `plan`, a whole sort whose
/// input's batches hold every column of its rows: its rows sorted, given
/// as batches.
///
/// # Errors
///
/// As `batches`; a spill's; a cancel; SQLCODE -910 of a sort of its first
/// rows alone, which gives none.
fn sorted_batches(plan: Plan, (db, tx): (&Arc<State>, TxId), _deferred: &Deferred) -> Result<Batches> {
    let Plan::Sort { keys, input, most: None, .. } = plan else { return Err(crate::exec::broken("a sort that gives no batches")) };
    let width = layout(&input, db).map_or(0, |(_, width)| width);
    crate::exec::sort::sorted_out(&keys, whole_batches(*input, db, tx)?, width, db, tx)
}

/// \[DESIGN\] (RULES.md D380) The batches of `plan`, the first rows of a
/// whole sort whose input's batches hold every column of its rows: those
/// rows, as the top-N makes them (`sort::first_batches`), made batches.
///
/// # Errors
///
/// As the top-N's rows; SQLCODE -910 of a plan not of a top-N whose
/// batches hold every column of its rows.
fn topped_batches(plan: Plan, (db, tx): (&Arc<State>, TxId), _deferred: &Deferred) -> Result<Batches> {
    let (_, width) = layout(&plan, db).ok_or_else(|| crate::exec::broken("a top-N that gives no batches"))?;
    let rows = crate::exec::execute(plan, db, tx).map(|row| row.map(|r| (crate::storage::RowId::default(), r)));
    let batches = crate::storage::batches_of(Box::new(rows), &(0..width).collect::<Vec<_>>());
    Ok(Box::new(batches.map(|b| b.map(|b| (b, None)))))
}

/// Of a plan that gives no batches: none.
///
/// # Errors
///
/// SQLCODE -910, always.
fn none(_plan: Plan, _at: (&Arc<State>, TxId), _deferred: &Deferred) -> Result<Batches> {
    Err(crate::exec::broken("a plan that gives no batches"))
}

/// The batches of `plan`, a subtree that gives them, its scans told the
/// filters `deferred` holds, each of `here` judged of the batches it
/// gives (RULES.md D369).
///
/// # Errors
///
/// As `open`.
fn judged_above(plan: Plan, (db, tx): (&Arc<State>, TxId), deferred: &Deferred, here: Vec<Told>) -> Result<Batches> {
    let columns = layout(&plan, db).map(|l| l.0).unwrap_or_default();
    Ok(runtime::applied(batches_with(plan, (db, tx), deferred)?, here, &columns, (db, false)))
}

/// The batches of `scan`, a table's scan: in an order if it is in one.
///
/// # Errors
///
/// As `batches`.
fn scan_batches(scan: Plan, (db, tx): (&Arc<State>, TxId), deferred: &Deferred) -> Result<Batches> {
    let Plan::SeqScan { table, columns, order, .. } = &scan else { return Err(crate::exec::broken("a scan is not one")) };
    if order.is_empty() { scan_of((table, columns, None), deferred, db, tx) } else { ordered_batches(scan, db, tx) }
}

/// The batches of `input` that `cond` keeps: of a scan's, its simple
/// conjuncts told the scan, unless it is in an order.
///
/// # Errors
///
/// As `batches`.
fn filter_batches(plan: Plan, (db, tx): (&Arc<State>, TxId), deferred: &Deferred) -> Result<Batches> {
    let Plan::Filter { cond, input, .. } = plan else { return Err(crate::exec::broken("a filter is not one")) };
    let below = deferred.filtered(&cond).with_told(deferred.filters().to_vec());
    let (inner, columns) = told(*input, &cond, (db, tx), &below)?;
    Ok(filtered(inner, (Judging::of(&cond, &columns), Runner::new(db, tx))))
}

/// The batches of `inner`, the input of a filter of `cond`, its scans'
/// columns `below` leaves unread given late, and the columns they hold:
/// of a scan's, its simple conjuncts told the scan, unless it is in an
/// order; \[EXTENSION\] of a counted scan's too, counted (RULES.md D378).
///
/// # Errors
///
/// As `batches`.
fn told(inner: Plan, cond: &Cond, (db, tx): (&Arc<State>, TxId), below: &Deferred) -> Result<(Batches, Vec<usize>)> {
    if let Plan::Counted(c) = inner {
        return super::counted::through(*c, db, |input| told(input, cond, (db, tx), below));
    }
    if let Plan::SeqScan { table, columns, order, .. } = &inner
        && order.is_empty()
    {
        return Ok((scan_of((table, columns, Some(cond)), below, db, tx)?, columns.clone()));
    }
    let columns = layout(&inner, db).map(|l| l.0).unwrap_or_default();
    Ok((batches_with(inner, (db, tx), below)?, columns))
}

/// The batches of the projection of `input` by `exprs`.
///
/// # Errors
///
/// As `batches`.
fn projected_batches(plan: Plan, (db, tx): (&Arc<State>, TxId), deferred: &Deferred) -> Result<Batches> {
    let Plan::Project { exprs, input, .. } = plan else { return Err(crate::exec::broken("a projection is not one")) };
    let input = *input;
    let columns = layout(&input, db).map(|l| l.0).unwrap_or_default();
    let below = if deferred.any() { deferred.projected(&exprs) } else { Deferred::default() };
    let below = below.with_told(runtime::projected(&exprs, deferred.filters()));
    let inner = batches_with(input, (db, tx), &below)?;
    if let Some(picks) = picked(&exprs, &columns) {
        return Ok(Box::new(inner.map(move |kept| kept.map(|(batch, rows)| (pick(batch, &picks), rows)))));
    }
    let places = exprs.iter().map(|e| place_of(e, &columns)).collect::<Option<Vec<_>>>();
    let places = places.ok_or_else(|| crate::exec::broken("a projection that gives no batches"))?;
    Ok(computed(inner, places, Runner::new(db, tx)))
}

/// The batches of `chain`, appended streams: each operand opened once
/// those before it are read, an error opening one given in its place.
///
/// # Errors
///
/// SQLCODE -910 if `chain` is no append.
fn appended_batches(chain: Plan, (db, tx): (&Arc<State>, TxId), _deferred: &Deferred) -> Result<Batches> {
    if !matches!(chain, Plan::Append { .. }) {
        return Err(crate::exec::broken("an append is not one"));
    }
    let later = Arc::clone(db);
    let operands = appended_owned(chain).into_iter();
    Ok(Box::new(operands.flat_map(move |operand| batches(operand, &later, tx).unwrap_or_else(|e| Box::new(std::iter::once(Err(e)))))))
}

/// `batch` with the vectors at `picks`, in order: each moved, or copied if
/// a later pick takes it too.
pub(super) fn pick(batch: Batch, picks: &[usize]) -> Batch {
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

/// A scan's batches (`scanned`), its columns `deferred` leaves unread
/// given late (RULES.md D361), told the bound of a sort above if it has
/// one (D379) and the ranges of the runtime filters it is told as
/// conjuncts, then judged by each of those filters, no row of them
/// filtered by the condition yet (RULES.md D369).
fn scan_of(scan: (&QualifiedName, &[usize], Option<&Cond>), deferred: &Deferred, db: &Arc<State>, tx: TxId) -> Result<Batches> {
    let storage = Arc::clone(db.storage_of(scan.0)?);
    let mut pushed = scan.2.and_then(pushed_conjuncts);
    let told = deferred.filters();
    let mut more: Vec<Conjunct> = told.iter().flat_map(|t| t.filter.range_conjuncts(t.column)).collect();
    // A sort's bound told the scan beside the filter's (RULES.md D379).
    more.extend(deferred.told());
    if !more.is_empty() {
        pushed.get_or_insert_with(|| Conjunction(Vec::new())).0.extend(more);
    }
    let batches = storage.scan_batches(tx, scan.1, pushed.as_ref(), &deferred.late_of(scan.1));
    let batches: Batches = Box::new(checked(batches, db.tx.stop_of(tx)).map(|b| b.map(|b| (b, None))));
    Ok(runtime::applied(batches, told.to_vec(), scan.1, (db, true)))
}

/// `inner`'s batches, each with the rows of those kept that `judging`
/// keeps. A batch whose judgment raises an error gives first the rows a
/// filter read row by row would give before it (`Judging::in_order_of`),
/// then the error of the first row that raises one, so a reader that
/// stops at one of those rows, as EXISTS does, sees what it would row by
/// row (RULES.md D15).
pub(super) fn filtered(inner: Batches, (judging, runner): (Judging, Runner)) -> Batches {
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
