//! \[DESIGN\] Set functions over a table read a batch at a time (RULES.md
//! D31, `TableStorage::scan_batches`): an aggregation whose set functions
//! each count rows or take a column, none DISTINCT, grouped by columns
//! the scan reads or not at all, of a table's scan, filtered or not. Each
//! function is given its column's values straight from the batch; under a
//! filter, a row is made only of the rows the scan's pushed condition
//! keeps, for the rest of the filter to judge. Groups are held in the work
//! area, each the grouping values and their set functions, given in the
//! order their first rows came; if they outgrow it, the aggregation is
//! grouped row by row instead, as is any other (`group.rs`), which spills.

use std::collections::HashMap;
use std::sync::Arc;

use super::group::STATE_BYTES;
use super::work::Work;
use super::{Runner, eval_cond, failed, simple_conjunct};
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::error::Result;
use crate::functions::Accumulator;
use crate::logical::{AggCall, Cond, Expr, Truth};
use crate::physical::Plan;
use crate::spill::{put_norm, row_bytes};
use crate::storage::{Batch, Row};
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
    /// Where each grouping column is in a batch.
    groups: Vec<usize>,
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
    let (scan, filter) =
        if let Plan::Filter { cond, input, .. } = input.as_ref() { (input.as_ref(), Some(cond)) } else { (input.as_ref(), None) };
    let Plan::SeqScan { table, columns, .. } = scan else { return None };
    let at = groups.iter().map(|g| columns.iter().position(|c| c == g)).collect::<Option<Vec<_>>>()?;
    Some(Read { table, columns, filter, taken: taken(aggs, columns)?, groups: at })
}

/// The row of the aggregation `plan`, read by batches, if it is one read
/// so; else none, for it to be grouped row by row.
pub(super) fn aggregated(plan: &Plan, db: &Arc<State>, tx: TxId) -> Option<super::RowIter> {
    let read = read_of(plan)?;
    let Plan::Aggregate { aggs, .. } = plan else { return None };
    if read.groups.is_empty() {
        return Some(match read_all(&read, aggs, db, tx) {
            Ok(row) => Box::new(std::iter::once(Ok(row))),
            Err(e) => failed(e),
        });
    }
    match grouped_all(&read, aggs, db, tx) {
        Ok(Some(rows)) => Some(Box::new(rows.into_iter().map(Ok))),
        // Too many groups for the work area: grouped row by row.
        Ok(None) => None,
        Err(e) => Some(failed(e)),
    }
}

/// The batches `read` reads, as `tx` sees them, each read only while the
/// statement is not cancelled (RULES.md D44).
fn batches_of(read: &Read<'_>, db: &Arc<State>, tx: TxId) -> Result<impl Iterator<Item = Result<Batch>> + use<>> {
    let storage = Arc::clone(db.storage_of(read.table)?);
    let pushed = read.filter.and_then(simple_conjunct);
    Ok(checked(storage.scan_batches(tx, read.columns, pushed.as_ref()), db.tx.stop_of(tx)))
}

/// The groups of the rows `read` reads, each its grouping values and the
/// finished set functions `aggs`; none if they outgrow the work area.
fn grouped_all(read: &Read<'_>, aggs: &[AggCall], db: &Arc<State>, tx: TxId) -> Result<Option<Vec<Row>>> {
    let mut groups = Groups { found: HashMap::new(), held: Vec::new(), bytes: 0, limit: Work::new(db).limit(), key: Vec::new() };
    let runner = Runner::new(db, tx);
    let width = read.columns.iter().max().map_or(0, |&c| c.saturating_add(1));
    for batch in batches_of(read, db, tx)? {
        let batch = batch?;
        let rows = match read.filter {
            Some(cond) => Some(kept(&batch, (cond, &runner), (read, width))?),
            None => None,
        };
        if !groups.feed(&batch, rows.as_deref(), (read, aggs))? {
            return Ok(None);
        }
    }
    groups
        .held
        .into_iter()
        .map(|(mut row, states)| {
            states.iter().map(|s| s.finish()).collect::<Result<Vec<_>>>().map(|r| {
                row.extend(r);
                row
            })
        })
        .collect::<Result<_>>()
        .map(Some)
}

/// Feeds `states` one row, as `taken` says, `value` giving its value at
/// each place of the batch.
fn take_row<'b>(states: &mut [Box<dyn Accumulator>], taken: &[Taken], value: impl Fn(usize) -> Option<&'b Value>) -> Result<()> {
    for (state, take) in states.iter_mut().zip(taken) {
        match *take {
            // [X3.135-1986 §5.8 GR4b] COUNT(*) counts rows.
            Taken::Count => state.update(&Value::Null)?,
            // [X3.135-1986 §5.8 GR1] nulls taken out.
            Taken::Column(c) => {
                if let Some(v) = value(c).filter(|v| !v.is_null()) {
                    state.update(v)?;
                }
            }
        }
    }
    Ok(())
}

/// Groups being made, in the work area.
struct Groups {
    /// Each group's place in `held`, by its grouping key.
    found: HashMap<Vec<u8>, usize>,
    /// Each group's grouping values and set functions, in the order their
    /// first rows came.
    held: Vec<(Row, Vec<Box<dyn Accumulator>>)>,
    bytes: usize,
    limit: usize,
    /// The key of the row being grouped.
    key: Vec<u8>,
}

impl Groups {
    /// Feeds the rows of `batch` at `rows`, or all, to their groups'
    /// set functions `aggs`, as `read` says; false if a new group would
    /// outgrow the work area.
    fn feed(&mut self, batch: &Batch, rows: Option<&[usize]>, (read, aggs): (&Read<'_>, &[AggCall])) -> Result<bool> {
        for n in 0..rows.map_or(batch.rows, <[usize]>::len) {
            let r = rows.map_or(Some(n), |rows| rows.get(n).copied()).unwrap_or(n);
            let value = |at: usize| batch.columns.get(at).and_then(|v| v.get(r));
            // The key made in a buffer kept, no value copied unless its
            // group is new.
            self.key.clear();
            for &at in &read.groups {
                put_norm(value(at).unwrap_or(&Value::Null), &mut self.key);
            }
            let at = if let Some(&at) = self.found.get(self.key.as_slice()) {
                at
            } else {
                let values: Row = read.groups.iter().map(|&at| value(at).cloned().unwrap_or(Value::Null)).collect();
                self.bytes = self
                    .bytes
                    .saturating_add(self.key.len())
                    .saturating_add(row_bytes(&values))
                    .saturating_add(aggs.len().saturating_mul(STATE_BYTES));
                if self.bytes > self.limit {
                    return Ok(false);
                }
                self.held.push((values, aggs.iter().map(|a| a.func.0.accumulator(a.result)).collect()));
                let at = self.held.len().saturating_sub(1);
                self.found.insert(self.key.clone(), at);
                at
            };
            let Some((_, states)) = self.held.get_mut(at) else { continue };
            take_row(states, &read.taken, value)?;
        }
        Ok(true)
    }
}

/// The finished set functions `aggs` of the rows `read` reads.
fn read_all(read: &Read<'_>, aggs: &[AggCall], db: &Arc<State>, tx: TxId) -> Result<Vec<Value>> {
    let batches = batches_of(read, db, tx)?;
    let mut states: Vec<Box<dyn Accumulator>> = aggs.iter().map(|a| a.func.0.accumulator(a.result)).collect();
    let runner = Runner::new(db, tx);
    // A row of the table's width, for the filter to judge.
    let width = read.columns.iter().max().map_or(0, |&c| c.saturating_add(1));
    for batch in batches {
        let batch = batch?;
        let rows = match read.filter {
            Some(cond) => Some(kept(&batch, (cond, &runner), (read, width))?),
            None => None,
        };
        fed(&batch, &read.taken, rows.as_deref(), &mut states)?;
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

/// The rows of `batch` the filter `cond` keeps, each made as a row of the
/// table's `width` to be judged.
fn kept(batch: &Batch, (cond, runner): (&Cond, &Runner), (read, width): (&Read<'_>, usize)) -> Result<Vec<usize>> {
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
    Ok(kept)
}
