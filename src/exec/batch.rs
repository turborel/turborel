//! \[DESIGN\] Set functions over tables read a batch at a time (RULES.md
//! D31, `TableStorage::scan_batches`): an aggregation whose set functions
//! each count rows or take an expression of columns the batches hold, no
//! subquery in it, none DISTINCT, grouped by such columns or not at all,
//! of a subtree that gives batches (`stream.rs`); and DISTINCT, as a
//! grouping by every column (`distinct_grouping`). Projections are read
//! so too (`project.rs`). Each function is given its column's values
//! straight from the batch, an expression's evaluated a vector at a time
//! where it can be (`vector.rs`); under a filter, only of the rows it
//! keeps (`judge.rs`). Groups are held in the work area, each the grouping
//! values and their set functions, given in the order their first rows
//! came; the rows of groups past it are written by batch, each partition
//! grouped again by batch, and merged by number with those held
//! (`groups.rs`). A null a set function's argument takes out raises its
//! warning [ISO/IEC 9075:1992 §6.5 GR1b], as a row at a time does. The
//! rows of a stream are read whole so too by a sort, EXCEPT and INTERSECT
//! (`keyed.rs`).

mod groups;
mod joined;
mod judge;
mod keyed;
mod project;
mod stream;
mod vector;

use std::sync::Arc;

pub(super) use keyed::{Keyed, KeyedBatches, kept, keyed, put_key, whole_width};
use project::Place;
pub(super) use project::{projectable, projected};
pub(super) use stream::Batches;
use vector::Lane;

use super::{Runner, eval, failed, warn};
use crate::db::State;
use crate::error::{Result, SqlState};
use crate::functions::Accumulator;
use crate::logical::{AggCall, Expr};
use crate::physical::Plan;
use crate::storage::{Batch, Cell, Column, Floats, Ints, Row, Texts};
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

/// What an aggregation read by batches reads: what each set function
/// takes, and where each grouping column is in a batch.
struct Read {
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
    let (columns, _) = stream::layout(input, db)?;
    let at = groups.iter().map(|g| columns.iter().position(|c| c == g)).collect::<Option<Vec<_>>>()?;
    Some(Read { taken: taken(aggs, &columns)?, groups: at })
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
    whole_width(input, db)
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
        return match read_all(&read, &aggs, stream.batches, &Runner::new(db, tx)) {
            Ok(row) => Box::new(std::iter::once(Ok(row))),
            Err(e) => failed(e),
        };
    }
    groups::grouped_all(&read, &aggs, (db, tx), stream.batches).unwrap_or_else(failed)
}

/// Feeds `states` the rows of one group of `batch`, as `taken` says: the
/// `given`th of those given, the rows `read_rows` of the batch; each set
/// function a vector of them at a time, of values or of integers, but of
/// an expression not evaluated by vectors, a row at a time.
fn fed_group(
    states: &mut [Box<dyn Accumulator>],
    (taken, scratch, runner): (&[Taken], &mut Row, &Runner),
    (batch, lanes): (&Batch, &[Option<Fed<'_>>]),
    (given, read_rows): (&[usize], &[usize]),
) -> Result<()> {
    for ((state, take), lane) in states.iter_mut().zip(taken).zip(lanes) {
        eliminated(group_nulled((take, lane), batch, (given, read_rows)), runner);
        match (take, lane) {
            // [X3.135-1986 §5.8 GR4b] COUNT(*) counts rows.
            (Taken::Count, _) => state.update_rows(given.len())?,
            // [X3.135-1986 §5.8 GR1] nulls taken out.
            (Taken::Column(c), _) => match batch.columns.get(*c) {
                Some(Column::Ints(ints)) => state.update_ints(ints, Some(read_rows))?,
                Some(Column::Floats(floats)) => state.update_floats(floats, Some(read_rows))?,
                Some(Column::Values(values)) => state.update_many(values, Some(read_rows))?,
                Some(Column::Texts(texts)) => state.update_texts(texts, Some(read_rows))?,
                None => {}
            },
            (Taken::Expr(..), Some(Fed::Ints(ints))) => state.update_ints(ints, Some(given))?,
            (Taken::Expr(..), Some(Fed::Values(Lane::Made(values)))) => state.update_many(values, Some(given))?,
            (Taken::Expr(..), Some(Fed::Values(Lane::Column(values, _)))) => state.update_many(values, Some(read_rows))?,
            (Taken::Expr(..), Some(Fed::Values(Lane::Const(v)))) => {
                if !v.is_null() {
                    (0..given.len()).try_for_each(|_| state.update(v))?;
                }
            }
            (Taken::Expr(e, inputs), None) => {
                for &r in read_rows {
                    let v = evaluated((e, inputs), (scratch, runner), |at, slot| put_cell(batch.cell(at, r), slot))?;
                    eliminated(v.is_null(), runner);
                    if !v.is_null() {
                        state.update(&v)?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// The value of `e`, of a row of the values at `inputs`, as `put` writes
/// them, made in `scratch`, its values written over; its warnings heard
/// by `runner`.
fn evaluated((e, inputs): (&Expr, &[usize]), (scratch, runner): (&mut Row, &Runner), put: impl Fn(usize, &mut Value)) -> Result<Value> {
    scratch.resize(inputs.len(), Value::Null);
    for (slot, &at) in scratch.iter_mut().zip(inputs) {
        put(at, slot);
    }
    eval(e, scratch, runner)
}

/// [ISO/IEC 9075:1992 §6.5 GR1b] Raises the warning null value eliminated
/// in set function, heard by `runner`, if a set function's argument took
/// out a null: `any`.
fn eliminated(any: bool, runner: &Runner) {
    if any {
        warn(runner, SqlState::NULL_VALUE_ELIMINATED);
    }
}

/// Whether any of `values` at `rows`, or any at all, is null.
fn values_nulled(values: &[Value], rows: Option<&[usize]>) -> bool {
    match rows {
        Some(rows) => rows.iter().any(|&r| values.get(r).is_some_and(Value::is_null)),
        None => values.iter().any(Value::is_null),
    }
}

/// Whether any of the rows `rows`, or any at all, is null, as `nulls`
/// marks them, if any is.
fn marked(nulls: Option<&[bool]>, rows: Option<&[usize]>) -> bool {
    nulls.is_some_and(|marks| match rows {
        Some(rows) => rows.iter().any(|&r| marks.get(r).copied().unwrap_or(false)),
        None => marks.contains(&true),
    })
}

/// Whether any of the rows `rows` of `column`, or any at all, is null.
fn column_nulled(column: &Column, rows: Option<&[usize]>) -> bool {
    match column {
        Column::Values(values) => values_nulled(values, rows),
        Column::Ints(Ints { nulls, .. }) | Column::Floats(Floats { nulls, .. }) | Column::Texts(Texts { nulls, .. }) => {
            marked(nulls.as_deref(), rows)
        }
    }
}

/// Whether the value of `lane` of any of the rows given `given` is null.
fn lane_nulled(lane: &Lane<'_>, mut given: impl Iterator<Item = usize>) -> bool {
    given.any(|n| lane.at(n).is_null())
}

/// Whether a set function that takes `take`, of `lane` if evaluated by
/// vectors, takes a null of the rows of one group of `batch`: the `given`th
/// of those given, the rows `read_rows` of the batch. One evaluated a row
/// at a time is told as it is.
fn group_nulled((take, lane): (&Taken, &Option<Fed<'_>>), batch: &Batch, (given, read_rows): (&[usize], &[usize])) -> bool {
    match (take, lane) {
        (Taken::Count, _) | (Taken::Expr(..), None) => false,
        (Taken::Column(c), _) => batch.columns.get(*c).is_some_and(|column| column_nulled(column, Some(read_rows))),
        (Taken::Expr(..), Some(Fed::Ints(ints))) => marked(ints.nulls.as_deref(), Some(given)),
        (Taken::Expr(..), Some(Fed::Values(lane))) => lane_nulled(lane, given.iter().copied()),
    }
}

/// Writes `cell`'s value over `slot`.
fn put_cell(cell: Cell<'_>, slot: &mut Value) {
    match cell {
        Cell::Value(v) => slot.clone_from(v),
        made @ (Cell::Exact(..) | Cell::Approx(_) | Cell::Text(_)) => *slot = made.to_value(),
        Cell::Null => *slot = Value::Null,
    }
}

/// The finished set functions `aggs` of the rows of `batches`, as `read`
/// says, their warnings heard by `runner`.
fn read_all(read: &Read, aggs: &[AggCall], batches: Batches, runner: &Runner) -> Result<Vec<Value>> {
    let mut states: Vec<Box<dyn Accumulator>> = aggs.iter().map(|a| a.func.0.accumulator(a.result)).collect();
    for kept in batches {
        let (batch, rows) = kept?;
        fed(&batch, (&read.taken, runner), rows.as_deref(), &mut states)?;
    }
    states.iter().map(|s| s.finish()).collect()
}

/// Feeds `states`, as `taken` says, the rows of `batch` at `rows`, or all:
/// each a vector at a time (`Accumulator::update_many`, `update_rows`);
/// the warnings raised heard by `runner`.
fn fed(batch: &Batch, (taken, runner): (&[Taken], &Runner), rows: Option<&[usize]>, states: &mut [Box<dyn Accumulator>]) -> Result<()> {
    for (state, take) in states.iter_mut().zip(taken) {
        match take {
            // [X3.135-1986 §5.8 GR4b] COUNT(*) counts rows.
            Taken::Count => state.update_rows(rows.map_or(batch.rows, <[usize]>::len))?,
            // [X3.135-1986 §5.8 GR1] nulls taken out.
            Taken::Column(at) => {
                eliminated(batch.columns.get(*at).is_some_and(|column| column_nulled(column, rows)), runner);
                match batch.columns.get(*at) {
                    Some(Column::Ints(ints)) => state.update_ints(ints, rows)?,
                    Some(Column::Floats(floats)) => state.update_floats(floats, rows)?,
                    Some(Column::Values(values)) => state.update_many(values, rows)?,
                    Some(Column::Texts(texts)) => state.update_texts(texts, rows)?,
                    None => state.update_many(&[], rows)?,
                }
            }
            // Its values evaluated a vector at a time (`vector.rs`), or else
            // a row at a time.
            Taken::Expr(e, inputs) => match vector::ints::lane(e, inputs, batch, rows) {
                // Of integers, as integers (`vector/ints.rs`).
                Some(ints) => {
                    eliminated(marked(ints.nulls.as_deref(), None), runner);
                    state.update_ints(&ints, None)?;
                }
                None => fed_by_vector(batch, (e, inputs, runner), rows, state)?,
            },
        }
    }
    Ok(())
}

/// Feeds `state` the values of `e` of the rows `rows` of `batch`, or all,
/// evaluated a vector at a time (`vector.rs`), or else a row at a time;
/// the warnings raised heard by `runner`.
fn fed_by_vector(
    batch: &Batch,
    (e, inputs, runner): (&Expr, &[usize], &Runner),
    rows: Option<&[usize]>,
    state: &mut Box<dyn Accumulator>,
) -> Result<()> {
    let count = rows.map_or(batch.rows, <[usize]>::len);
    let Some(lane) = vector::lane(e, inputs, batch, rows) else { return fed_by_rows(batch, (e, inputs, runner), rows, state) };
    eliminated(lane_nulled(&lane, 0..count), runner);
    match lane {
        Lane::Column(values, given) => state.update_many(values, given),
        Lane::Made(values) => state.update_many(&values, None),
        Lane::Const(v) => state.update_many(&vec![v.clone(); count], None),
    }
}

/// Feeds `state` the values of `e` of the rows `rows` of `batch`, or all,
/// each evaluated as a row's is, its columns the vectors at `inputs`; the
/// warnings raised heard by `runner`.
fn fed_by_rows(
    batch: &Batch,
    (e, inputs, runner): (&Expr, &[usize], &Runner),
    rows: Option<&[usize]>,
    state: &mut Box<dyn Accumulator>,
) -> Result<()> {
    let mut scratch = Vec::new();
    let value = |at: usize, r: usize| batch.value(at, r).unwrap_or(&Value::Null);
    let mut values = Vec::with_capacity(rows.map_or(batch.rows, <[usize]>::len));
    for r in rows.map_or_else(|| (0..batch.rows).collect(), <[usize]>::to_vec) {
        values.push(evaluated((e, inputs), (&mut scratch, runner), |at, slot| slot.clone_from(value(at, r)))?);
    }
    eliminated(values_nulled(&values, None), runner);
    state.update_many(&values, None)
}

/// The values of each of `taken` that is an expression of the rows `rows`
/// of `batch`, or all, evaluated a vector at a time, where it is
/// (`vector.rs`).
fn lanes<'b>(taken: &'b [Taken], batch: &'b Batch, rows: Option<&'b [usize]>) -> Vec<Option<Fed<'b>>> {
    let fed = |e: &'b Expr, inputs: &'b [usize]| {
        vector::ints::lane(e, inputs, batch, rows).map(Fed::Ints).or_else(|| vector::lane(e, inputs, batch, rows).map(Fed::Values))
    };
    taken.iter().map(|t| if let Taken::Expr(e, inputs) = t { fed(e, inputs) } else { None }).collect()
}

/// An expression's values of a batch, evaluated a vector at a time: as
/// values, or as integers.
enum Fed<'b> {
    Values(Lane<'b>),
    Ints(Ints),
}
