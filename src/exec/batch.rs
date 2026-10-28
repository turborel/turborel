//! \[DESIGN\] Set functions over a table read a batch at a time (RULES.md
//! D31, `TableStorage::scan_batches`): an aggregation with no GROUP BY,
//! whose set functions each count rows or take a column, none DISTINCT,
//! of a table's scan, filtered or not. Each function is given its column's
//! values straight from the batch; under a filter, a row is made only of
//! the rows the scan's pushed condition keeps, for the rest of the filter
//! to judge. Any other aggregation is grouped row by row (`group.rs`).

use std::sync::Arc;

use super::{Runner, eval_cond, failed, simple_conjunct};
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::error::Result;
use crate::functions::Accumulator;
use crate::logical::{AggCall, Cond, Expr, Truth};
use crate::physical::Plan;
use crate::storage::Batch;
use crate::txn::{TxId, checked};
use crate::value::Value;

/// What a set function takes of each row: a count of it, or the value of
/// the column at this place of the batch.
#[derive(Debug, Clone, Copy)]
enum Taken {
    Count,
    Column(usize),
}

/// What an aggregation read by batches reads: its table, the columns the
/// scan reads, its filter if any, and what each set function takes.
struct Read<'p> {
    table: &'p QualifiedName,
    columns: &'p [usize],
    filter: Option<&'p Cond>,
    taken: Vec<Taken>,
}

/// What each of `aggs` takes of a batch of `columns`, if each counts rows
/// or takes a column the scan reads, and none is DISTINCT.
fn taken(aggs: &[AggCall], columns: &[usize]) -> Option<Vec<Taken>> {
    aggs.iter()
        .map(|a| match (&a.arg, a.distinct) {
            (None, false) => Some(Taken::Count),
            (Some(Expr::Column(c)), false) => columns.iter().position(|x| x == c).map(Taken::Column),
            (Some(_), _) | (None, true) => None,
        })
        .collect()
}

/// What `plan` reads, if it is an aggregation read by batches.
fn read_of(plan: &Plan) -> Option<Read<'_>> {
    let Plan::Aggregate { groups, aggs, input, .. } = plan else { return None };
    if !groups.is_empty() {
        return None;
    }
    let (scan, filter) =
        if let Plan::Filter { cond, input, .. } = input.as_ref() { (input.as_ref(), Some(cond)) } else { (input.as_ref(), None) };
    let Plan::SeqScan { table, columns, .. } = scan else { return None };
    Some(Read { table, columns, filter, taken: taken(aggs, columns)? })
}

/// The row of the aggregation `plan`, read by batches, if it is one read
/// so; else none, for it to be grouped row by row.
pub(super) fn aggregated(plan: &Plan, db: &Arc<State>, tx: TxId) -> Option<super::RowIter> {
    let read = read_of(plan)?;
    let Plan::Aggregate { aggs, .. } = plan else { return None };
    let row = read_all(&read, aggs, db, tx);
    Some(match row {
        Ok(row) => Box::new(std::iter::once(Ok(row))),
        Err(e) => failed(e),
    })
}

/// The finished set functions `aggs` of the rows `read` reads.
fn read_all(read: &Read<'_>, aggs: &[AggCall], db: &Arc<State>, tx: TxId) -> Result<Vec<Value>> {
    let storage = Arc::clone(db.storage_of(read.table)?);
    let pushed = read.filter.and_then(simple_conjunct);
    let batches = checked(storage.scan_batches(tx, read.columns, pushed.as_ref()), db.tx.stop_of(tx));
    let mut states: Vec<Box<dyn Accumulator>> = aggs.iter().map(|a| a.func.0.accumulator(a.result)).collect();
    let runner = Runner::new(db, tx);
    // A row of the table's width, for the filter to judge.
    let width = read.columns.iter().max().map_or(0, |&c| c.saturating_add(1));
    for batch in batches {
        let batch = batch?;
        match read.filter {
            Some(cond) => filtered(&batch, (cond, &runner), (read, width), &mut states)?,
            None => fed(&batch, &read.taken, None, &mut states)?,
        }
    }
    states.iter().map(|s| s.finish()).collect()
}

/// Feeds `states`, as `taken` says, the rows of `batch` at `rows`, or all.
fn fed(batch: &Batch, taken: &[Taken], rows: Option<&[usize]>, states: &mut [Box<dyn Accumulator>]) -> Result<()> {
    for (state, take) in states.iter_mut().zip(taken) {
        match (*take, rows) {
            // [X3.135-1986 §5.8 GR4b] COUNT(*) counts rows.
            (Taken::Count, rows) => {
                for _ in 0..rows.map_or(batch.rows, <[usize]>::len) {
                    state.update(&Value::Null)?;
                }
            }
            // [X3.135-1986 §5.8 GR1] nulls taken out.
            (Taken::Column(at), None) => {
                for v in batch.columns.get(at).into_iter().flatten().filter(|v| !v.is_null()) {
                    state.update(v)?;
                }
            }
            (Taken::Column(at), Some(rows)) => {
                let vector = batch.columns.get(at).map_or(&[][..], Vec::as_slice);
                for v in rows.iter().filter_map(|&r| vector.get(r)).filter(|v| !v.is_null()) {
                    state.update(v)?;
                }
            }
        }
    }
    Ok(())
}

/// Feeds `states` the rows of `batch` the filter `cond` keeps, each made
/// as a row of the table's `width` to be judged.
fn filtered(
    batch: &Batch,
    (cond, runner): (&Cond, &Runner),
    (read, width): (&Read<'_>, usize),
    states: &mut [Box<dyn Accumulator>],
) -> Result<()> {
    let mut kept = Vec::new();
    let mut row = vec![Value::Null; width];
    for r in 0..batch.rows {
        for (vector, &c) in batch.columns.iter().zip(read.columns) {
            if let (Some(slot), Some(v)) = (row.get_mut(c), vector.get(r)) {
                slot.clone_from(v);
            }
        }
        if eval_cond(cond, &row, runner)? == Truth::True {
            kept.push(r);
        }
    }
    fed(batch, &read.taken, Some(&kept), states)
}
