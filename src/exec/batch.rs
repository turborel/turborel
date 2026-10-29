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

mod judge;
mod project;
mod stream;
mod vector;

use std::collections::HashMap;
use std::sync::Arc;

use project::Place;
pub(super) use project::{projectable, projected};
use vector::Lane;

use super::group::{Finished, STATE_BYTES, Spec, Spilling};
use super::work::Work;
use super::{NoSubqueries, eval, failed};
use crate::db::State;
use crate::error::Result;
use crate::functions::Accumulator;
use crate::logical::{AggCall, Expr};
use crate::physical::Plan;
use crate::spill::{put_norm, row_bytes};
use crate::storage::{Batch, Column, Row};
use crate::txn::TxId;
use crate::value::Value;

/// What a set function takes of each row: a count of it, the value of the
/// column at this place of the batch, or that of an expression of a row of
/// the values at these places, its columns those of the row
/// (`project::place_of`).
#[derive(Debug, Clone)]
enum Taken {
    Count,
    Column(usize),
    Expr(Expr, Vec<usize>),
}

/// What an aggregation read by batches reads: the columns of its input's
/// rows its batches hold, the width of those rows, what each set function
/// takes, and where each grouping column is in a batch.
struct Read {
    columns: Vec<usize>,
    width: usize,
    taken: Vec<Taken>,
    groups: Vec<usize>,
}

/// What each of `aggs` takes of a batch of `columns`, if each counts rows
/// or takes an expression of columns the batch holds, no subquery in it,
/// and none is DISTINCT.
fn taken(aggs: &[AggCall], columns: &[usize]) -> Option<Vec<Taken>> {
    aggs.iter()
        .map(|a| match (&a.arg, a.distinct) {
            (None, false) => Some(Taken::Count),
            (Some(e), false) => Some(match project::place_of(e, columns)? {
                Place::Column(at) => Taken::Column(at),
                Place::Const(v) => Taken::Expr(Expr::Const(v), Vec::new()),
                Place::Expr(e, inputs) => Taken::Expr(e, inputs),
            }),
            (Some(_), true) | (None, true) => None,
        })
        .collect()
}

/// What `plan` reads, if it is an aggregation read by batches: of a
/// subtree that gives them (`stream::layout`).
fn read_of(plan: &Plan, db: &State) -> Option<Read> {
    let Plan::Aggregate { groups, aggs, input, .. } = plan else { return None };
    let (columns, width) = stream::layout(input, db)?;
    let at = groups.iter().map(|g| columns.iter().position(|c| c == g)).collect::<Option<Vec<_>>>()?;
    Some(Read { taken: taken(aggs, &columns)?, groups: at, columns, width })
}

/// \[DESIGN\] The width of the rows of the DISTINCT `plan`, if it is a
/// grouping of them by every column, no set function (`distinct_grouping`): of a
/// stream whose batches hold every column of its rows (`stream::layout`), a
/// projection of columns, a UNION's operands. Never inlined into the
/// caller, whose frame every level of a plan adds to the stack (RULES.md
/// R-CF-6).
#[inline(never)]
pub(super) fn distinct_groupable(plan: &Plan, db: &State) -> Option<usize> {
    let Plan::Distinct { input } = plan else { return None };
    stream::layout(input, db).filter(|(columns, width)| columns.iter().copied().eq(0..*width)).map(|l| l.1)
}

/// \[DESIGN\] The DISTINCT `plan` as the grouping of its rows by every
/// column of them, of `width`, as `distinct_groupable` finds. The groups are its rows: one
/// of each set of duplicates, alike in every column [X3.135-1986 §5.25 GR2,
/// GR5, §8.3 GR2b], in the order their first rows came, as DISTINCT gives
/// them, none of no rows.
#[inline(never)]
pub(super) fn distinct_grouping(plan: Plan, width: usize) -> Plan {
    let Plan::Distinct { input } = plan else { return plan };
    Plan::Aggregate { groups: (0..width).collect(), aggs: Vec::new(), detail: String::new(), input }
}

/// Whether `plan` is an aggregation read by batches.
pub(super) fn aggregable(plan: &Plan, db: &State) -> bool {
    read_of(plan, db).is_some()
}

/// The rows of the aggregation `plan`, one read by batches (`aggregable`).
pub(super) fn aggregated(plan: Plan, db: &Arc<State>, tx: TxId) -> super::RowIter {
    let (Some(read), Plan::Aggregate { aggs, input, .. }) = (read_of(&plan, db), plan) else {
        return failed(super::broken("an aggregation not read by batches"));
    };
    let stream = match stream::open(*input, db, tx) {
        Ok(stream) => stream,
        Err(e) => return failed(e),
    };
    if read.groups.is_empty() {
        return match read_all(&read, &aggs, stream.batches) {
            Ok(row) => Box::new(std::iter::once(Ok(row))),
            Err(e) => failed(e),
        };
    }
    grouped_all(&read, &aggs, (db, tx), stream.batches).unwrap_or_else(failed)
}

/// The groups of the rows `read` reads, each its grouping values and the
/// finished set functions `aggs`, in the order their first rows came;
/// those past the work area made as `exec/group.rs` makes them.
fn grouped_all(read: &Read, aggs: &[AggCall], (db, tx): (&Arc<State>, TxId), batches: stream::Batches) -> Result<super::RowIter> {
    let width = read.width;
    let spec = Spec { groups: read.groups.iter().filter_map(|&at| read.columns.get(at).copied()).collect(), aggs: aggs.to_vec() };
    let mut groups = Groups {
        found: HashMap::new(),
        held: Vec::new(),
        bytes: 0,
        limit: Work::new(db).limit(),
        key: Vec::new(),
        rows: 0,
        spilling: Spilling::new(spec, db, tx),
        spilled: false,
        width,
        scratch: Vec::new(),
    };
    for kept in batches {
        let (batch, rows) = kept?;
        groups.feed(&batch, rows.as_deref(), (read, aggs))?;
    }
    let held: Finished = groups
        .held
        .into_iter()
        .map(|(first, mut row, states)| {
            for state in &states {
                row.push(state.finish()?);
            }
            Ok((first, row))
        })
        .collect::<Result<_>>()?;
    groups.spilling.finish(held)
}

/// Feeds `states` one row, as `taken` says, `value` giving its value at
/// each place of the batch.
fn take_row<'b>(
    states: &mut [Box<dyn Accumulator>],
    (taken, scratch): (&[Taken], &mut Row),
    value: impl Fn(usize) -> Option<&'b Value>,
    (lanes, n): (&[Option<Lane<'_>>], usize),
) -> Result<()> {
    for ((state, take), lane) in states.iter_mut().zip(taken).zip(lanes) {
        match take {
            // [X3.135-1986 §5.8 GR4b] COUNT(*) counts rows.
            Taken::Count => state.update(&Value::Null)?,
            // [X3.135-1986 §5.8 GR1] nulls taken out.
            Taken::Column(c) => {
                if let Some(v) = value(*c).filter(|v| !v.is_null()) {
                    state.update(v)?;
                }
            }
            // Its value of the batch's evaluated a vector at a time, if
            // it was (`vector.rs`).
            Taken::Expr(e, inputs) => {
                if let Some(lane) = lane {
                    let v = lane.at(n);
                    if !v.is_null() {
                        state.update(v)?;
                    }
                    continue;
                }
                let v = evaluated((e, inputs), scratch, |at| value(at).unwrap_or(&Value::Null))?;
                if !v.is_null() {
                    state.update(&v)?;
                }
            }
        }
    }
    Ok(())
}

/// The value of `e`, of a row of the values at `inputs`, as `value` gives
/// them, made in `scratch`, its values written over.
fn evaluated<'b>((e, inputs): (&Expr, &[usize]), scratch: &mut Row, value: impl Fn(usize) -> &'b Value) -> Result<Value> {
    scratch.resize(inputs.len(), Value::Null);
    for (slot, &at) in scratch.iter_mut().zip(inputs) {
        slot.clone_from(value(at));
    }
    eval(e, scratch, &NoSubqueries)
}

/// A group held: its first row's number, its grouping values and its set
/// functions.
type Held = (u64, Row, Vec<Box<dyn Accumulator>>);

/// Groups being made, in the work area.
struct Groups {
    /// Each group's place in `held`, by its grouping key.
    found: HashMap<Vec<u8>, usize>,
    /// Each group's first row's number, grouping values and set
    /// functions, in the order their first rows came.
    held: Vec<Held>,
    bytes: usize,
    limit: usize,
    /// The key of the row being grouped.
    key: Vec<u8>,
    /// The rows grouped so far, each numbered by where it came.
    rows: u64,
    /// Where the rows of groups past the work area go.
    spilling: Spilling,
    /// Whether a group has been past it: every group not yet held is then,
    /// so no group is both held and written.
    spilled: bool,
    /// The width of a table's row, as a row written is made.
    width: usize,
    /// The row an expression a set function takes is evaluated of.
    scratch: Row,
}

impl Groups {
    /// Feeds the rows of `batch` at `rows`, or all, to their groups'
    /// set functions `aggs`, as `read` says; a row of a group past the work
    /// area is written, for its group to be made after.
    fn feed(&mut self, batch: &Batch, rows: Option<&[usize]>, (read, aggs): (&Read, &[AggCall])) -> Result<()> {
        let lanes = lanes(&read.taken, batch, rows);
        for n in 0..rows.map_or(batch.rows, <[usize]>::len) {
            let r = rows.map_or(Some(n), |rows| rows.get(n).copied()).unwrap_or(n);
            let value = |at: usize| batch.value(at, r);
            // The key made in a buffer kept, no value copied unless its
            // group is new.
            self.key.clear();
            for &at in &read.groups {
                put_norm(value(at).unwrap_or(&Value::Null), &mut self.key);
            }
            let number = self.rows;
            self.rows = self.rows.saturating_add(1);
            let Some(at) = self.place(number, (batch, r), (read, aggs))? else { continue };
            let Some((_, _, states)) = self.held.get_mut(at) else { continue };
            take_row(states, (&read.taken, &mut self.scratch), value, (&lanes, n))?;
        }
        Ok(())
    }

    /// The place of the group of row `r` of `batch`, the `number`th, its
    /// key made: found, or made if the work area has room; else the row is
    /// written, and it has none.
    fn place(&mut self, number: u64, (batch, r): (&Batch, usize), (read, aggs): (&Read, &[AggCall])) -> Result<Option<usize>> {
        if let Some(&at) = self.found.get(self.key.as_slice()) {
            return Ok(Some(at));
        }
        let value = |at: usize| batch.value(at, r).cloned().unwrap_or(Value::Null);
        let values: Row = read.groups.iter().map(|&at| value(at)).collect();
        let bytes = self.key.len().saturating_add(row_bytes(&values)).saturating_add(aggs.len().saturating_mul(STATE_BYTES));
        if self.spilled || self.bytes.saturating_add(bytes) > self.limit {
            self.spilled = true;
            let mut row = vec![Value::Null; self.width];
            for (at, &c) in read.columns.iter().enumerate() {
                if let Some(slot) = row.get_mut(c) {
                    *slot = value(at);
                }
            }
            self.spilling.push(&self.key, (number, &row))?;
            return Ok(None);
        }
        self.bytes = self.bytes.saturating_add(bytes);
        self.held.push((number, values, aggs.iter().map(|a| a.func.0.accumulator(a.result)).collect()));
        let at = self.held.len().saturating_sub(1);
        self.found.insert(self.key.clone(), at);
        Ok(Some(at))
    }
}

/// The finished set functions `aggs` of the rows of `batches`, as `read`
/// says.
fn read_all(read: &Read, aggs: &[AggCall], batches: stream::Batches) -> Result<Vec<Value>> {
    let mut states: Vec<Box<dyn Accumulator>> = aggs.iter().map(|a| a.func.0.accumulator(a.result)).collect();
    for kept in batches {
        let (batch, rows) = kept?;
        fed(&batch, &read.taken, rows.as_deref(), &mut states)?;
    }
    states.iter().map(|s| s.finish()).collect()
}

/// Feeds `states`, as `taken` says, the rows of `batch` at `rows`, or all:
/// each a vector at a time (`Accumulator::update_many`, `update_rows`).
fn fed(batch: &Batch, taken: &[Taken], rows: Option<&[usize]>, states: &mut [Box<dyn Accumulator>]) -> Result<()> {
    for (state, take) in states.iter_mut().zip(taken) {
        match take {
            // [X3.135-1986 §5.8 GR4b] COUNT(*) counts rows.
            Taken::Count => state.update_rows(rows.map_or(batch.rows, <[usize]>::len))?,
            // [X3.135-1986 §5.8 GR1] nulls taken out.
            Taken::Column(at) => match batch.columns.get(*at) {
                Some(Column::Ints(ints)) => state.update_ints(ints, rows)?,
                Some(Column::Values(values)) => state.update_many(values, rows)?,
                None => state.update_many(&[], rows)?,
            },
            // Its values evaluated a vector at a time (`vector.rs`), or else
            // a row at a time.
            Taken::Expr(e, inputs) => match vector::lane(e, inputs, batch, rows) {
                Some(Lane::Column(values, given)) => state.update_many(values, given)?,
                Some(Lane::Made(values)) => state.update_many(&values, None)?,
                Some(Lane::Const(v)) => state.update_many(&vec![v.clone(); rows.map_or(batch.rows, <[usize]>::len)], None)?,
                None => fed_by_rows(batch, (e, inputs), rows, state)?,
            },
        }
    }
    Ok(())
}

/// Feeds `state` the values of `e` of the rows `rows` of `batch`, or all,
/// each evaluated as a row's is, its columns the vectors at `inputs`.
fn fed_by_rows(batch: &Batch, (e, inputs): (&Expr, &[usize]), rows: Option<&[usize]>, state: &mut Box<dyn Accumulator>) -> Result<()> {
    let mut scratch = Vec::new();
    let value = |at: usize, r: usize| batch.value(at, r).unwrap_or(&Value::Null);
    let mut values = Vec::with_capacity(rows.map_or(batch.rows, <[usize]>::len));
    for r in rows.map_or_else(|| (0..batch.rows).collect(), <[usize]>::to_vec) {
        values.push(evaluated((e, inputs), &mut scratch, |at| value(at, r))?);
    }
    state.update_many(&values, None)
}

/// The values of each of `taken` that is an expression of the rows `rows`
/// of `batch`, or all, evaluated a vector at a time, where it is
/// (`vector.rs`).
fn lanes<'b>(taken: &'b [Taken], batch: &'b Batch, rows: Option<&'b [usize]>) -> Vec<Option<Lane<'b>>> {
    taken.iter().map(|t| if let Taken::Expr(e, inputs) = t { vector::lane(e, inputs, batch, rows) } else { None }).collect()
}
