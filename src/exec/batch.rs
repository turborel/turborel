//! \[DESIGN\] Set functions over tables read a batch at a time (RULES.md
//! D31, `TableStorage::scan_batches`): an aggregation whose set functions
//! each count rows or take an expression of columns the batches hold, no
//! subquery in it, none DISTINCT, grouped by such columns or not at all,
//! of a subtree that gives batches (`stream.rs`); and DISTINCT, as a
//! grouping by every column (`distinct_grouping`). Projections are read
//! so too (`project.rs`). Each function is given its column's values
//! straight from the batch, an expression's evaluated a vector at a time
//! where it can be (`expr.rs`, RULES.md D362); under a filter, only of the rows it
//! keeps (`judge.rs`). Groups are held in the work area, each the grouping
//! values and their set functions, given in the order their first rows
//! came; groups past it are made partly before they spill, if their set
//! functions write their states, else their rows are written by batch;
//! each partition grouped again by batch, and merged by number with those
//! held (`groups.rs`). A null a set function's argument takes out raises its
//! warning [ISO/IEC 9075:1992 §6.5 GR1b], as a row at a time does. The
//! rows of a stream are read whole so too by a sort, EXCEPT and INTERSECT
//! (`keyed.rs`).

mod count;
mod expr;
mod groups;
mod joined;
mod judge;
mod keyed;
mod late;
mod project;
mod shaped;
mod stream;

use std::sync::Arc;

pub(crate) use count::storage_counted;
use expr::Program;
pub(super) use groups::{Held, morsel_groups};
pub(super) use joined::rows_of;
pub(super) use keyed::{Keyed, KeyedBatches, kept, keyed, put_key, whole_width};
pub(super) use late::{Deferred, resolved};
use project::Place;
pub(super) use project::{projectable, projected};
pub(super) use shaped::{Shaped, filter_step, project_step, through};
pub(super) use stream::Batches;

use super::{Runner, eval, failed, warn};
use crate::db::State;
use crate::error::{Result, SqlState};
use crate::functions::Accumulator;
use crate::logical::{AggCall, Expr};
use crate::physical::Plan;
use crate::storage::mask::index;
use crate::storage::{Batch, Bools, Cell, Column, Datetimes, Floats, Ints, Mask, Row, Sel, Texts};
use crate::txn::TxId;
use crate::value::Value;

/// What a set function takes of each row: a count of it, the value of the
/// column at this place of the batch, or that of an expression of a row of
/// the values at some places, its columns those of the row, compiled for
/// vectors (`project::place_of`, `expr.rs`).
#[derive(Debug, Clone)]
enum Taken {
    Count,
    Column(usize),
    Expr(Arc<Program>),
}

/// What an aggregation read by batches reads: what each set function
/// takes, and where each grouping column is in a batch.
pub(in crate::exec) struct Read {
    taken: Vec<Taken>,
    groups: Vec<usize>,
}

/// What each of `aggs` takes of a batch of `columns`, if each counts rows
/// or takes an expression of columns the batch holds, no subquery in it,
/// and none is DISTINCT, filtered or an inverse distribution function,
/// which rows alone take.
fn taken(aggs: &[AggCall], columns: &[usize]) -> Option<Vec<Taken>> {
    aggs.iter()
        .map(|a| match (&a.arg, a.distinct || a.filter.is_some() || a.distribution.is_some()) {
            (None, false) => Some(Taken::Count),
            (Some(e), false) => Some(match project::place_of(e, columns)? {
                Place::Column(at) => Taken::Column(at),
                Place::Const(v) => Taken::Expr(Arc::new(Program::of(Expr::Const(v), Vec::new()))),
                Place::Expr(program) => Taken::Expr(program),
            }),
            (Some(_), true) | (None, true) => None,
        })
        .collect()
}

/// \[DESIGN\] (RULES.md D354) What a grouping by `groups`, columns of the
/// rows, computing `aggs`, reads of batches whose vectors hold the columns
/// `layout`, if it reads them so: each grouping column one of them, each
/// set function as `taken` admits.
pub(in crate::exec) fn read_for(groups: &[usize], aggs: &[AggCall], layout: &[usize]) -> Option<Read> {
    let at = groups.iter().map(|g| layout.iter().position(|c| c == g)).collect::<Option<Vec<_>>>()?;
    Some(Read { taken: taken(aggs, layout)?, groups: at })
}

/// What `plan` reads, if it is an aggregation read by batches: of a
/// subtree that gives them (`stream::layout`).
fn read_of(plan: &Plan, db: &State) -> Option<Read> {
    let Plan::Aggregate { groups, collations, aggs, seeds, input, .. } = plan else { return None };
    // Batches key groups by their values, not by a collation's sort keys,
    // and make none before their rows.
    if !collations.is_empty() || !seeds.is_empty() {
        return None;
    }
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
    let Plan::Distinct { input, .. } = plan else { return None };
    whole_width(input, db)
}

/// \[DESIGN\] The DISTINCT `plan` as the grouping of its rows by every
/// column of them, of `width`, as `distinct_groupable` finds. The groups are its rows: one
/// of each set of duplicates, alike in every column [X3.135-1986 §5.25 GR2,
/// GR5, §8.3 GR2b], in the order their first rows came, as DISTINCT gives
/// them, none of no rows.
#[inline(never)]
pub(super) fn distinct_grouping(plan: Plan, width: usize) -> Plan {
    let Plan::Distinct { input, est } = plan else { return plan };
    Plan::Aggregate {
        groups: (0..width).collect(),
        collations: Vec::new(),
        aggs: Vec::new(),
        seeds: Vec::new(),
        detail: String::new(),
        input,
        est,
    }
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
    // A bare COUNT(*), counted by the table's storage if it can (`count.rs`).
    if let Some(row) = count::counted_table(&read, &input).and_then(|table| count::counted(table, &aggs, db, tx)) {
        return Box::new(std::iter::once(row));
    }
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
    (given, read_rows): (&[u32], &[u32]),
) -> Result<()> {
    for ((state, take), lane) in states.iter_mut().zip(taken).zip(lanes) {
        eliminated(group_nulled((take, lane), batch, (given, read_rows)), runner);
        match (take, lane) {
            // [X3.135-1986 §5.8 GR4b] COUNT(*) counts rows.
            (Taken::Count, _) => state.update_rows(given.len())?,
            // [X3.135-1986 §5.8 GR1] nulls taken out.
            (Taken::Column(c), _) => {
                if let Some(column) = batch.columns.get(*c) {
                    fed_column(state, column, Some(read_rows))?;
                }
            }
            (Taken::Expr(_), Some(Fed::Made(column))) => fed_column(state, column, Some(given))?,
            (Taken::Expr(_), Some(Fed::Const(v))) => {
                if !v.is_null() {
                    (0..given.len()).try_for_each(|_| state.update(v))?;
                }
            }
            (Taken::Expr(program), None) => {
                for &r in read_rows {
                    let v = evaluated((&program.expr, &program.inputs), (scratch, runner), |at, slot| {
                        put_cell(batch.cell(at, index(r)), slot);
                    })?;
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
fn values_nulled(values: &[Value], rows: Option<&[u32]>) -> bool {
    match rows {
        Some(rows) => rows.iter().any(|&r| values.get(index(r)).is_some_and(Value::is_null)),
        None => values.iter().any(Value::is_null),
    }
}

/// Whether any of the rows `rows`, or any at all, is null, as `nulls`
/// marks them, if any is.
fn marked(nulls: Option<&Mask>, rows: Option<&[u32]>) -> bool {
    nulls.is_some_and(|marks| marks.any_of(rows))
}

/// Whether any of the rows `rows` of `column`, or any at all, is null.
fn column_nulled(column: &Column, rows: Option<&[u32]>) -> bool {
    match column {
        Column::Values(values) => values_nulled(values, rows),
        Column::Late(late) => values_nulled(late.values(), rows),
        Column::Ints(Ints { nulls, .. })
        | Column::Floats(Floats { nulls, .. })
        | Column::Texts(Texts { nulls, .. })
        | Column::Datetimes(Datetimes { nulls, .. })
        | Column::Bools(Bools { nulls, .. }) => marked(nulls.as_ref(), rows),
        Column::Nulls(nulls) => rows.map_or(nulls.rows > 0, |r| !r.is_empty()),
    }
}

/// Whether a set function that takes `take`, of `lane` if evaluated by
/// vectors, takes a null of the rows of one group of `batch`: the `given`th
/// of those given, the rows `read_rows` of the batch. One evaluated a row
/// at a time is told as it is.
fn group_nulled((take, lane): (&Taken, &Option<Fed<'_>>), batch: &Batch, (given, read_rows): (&[u32], &[u32])) -> bool {
    match (take, lane) {
        (Taken::Count, _) | (Taken::Expr(_), None) => false,
        (Taken::Column(c), _) => batch.columns.get(*c).is_some_and(|column| column_nulled(column, Some(read_rows))),
        (Taken::Expr(_), Some(Fed::Made(column))) => column_nulled(column, Some(given)),
        (Taken::Expr(_), Some(Fed::Const(v))) => v.is_null() && !given.is_empty(),
    }
}

/// Writes `cell`'s value over `slot`.
fn put_cell(cell: Cell<'_>, slot: &mut Value) {
    match cell {
        Cell::Value(v) => slot.clone_from(v),
        made @ (Cell::Exact(..) | Cell::Approx(_) | Cell::Text(_) | Cell::Datetime(_) | Cell::Bool(_)) => *slot = made.to_value(),
        Cell::Null => *slot = Value::Null,
    }
}

/// The finished set functions `aggs` of the rows of `batches`, as `read`
/// says, their warnings heard by `runner`.
fn read_all(read: &Read, aggs: &[AggCall], batches: Batches, runner: &Runner) -> Result<Vec<Value>> {
    let mut states: Vec<Box<dyn Accumulator>> = aggs.iter().map(|a| a.func.0.accumulator(a.result.clone())).collect();
    for kept in batches {
        let (batch, rows) = kept?;
        fed(&batch, (&read.taken, runner), rows.as_ref().map(Sel::as_slice), &mut states)?;
    }
    states.iter().map(|s| s.finish()).collect()
}

/// Feeds `states`, as `taken` says, the rows of `batch` at `rows`, or all:
/// each a vector at a time (`Accumulator::update_many`, `update_rows`);
/// the warnings raised heard by `runner`.
fn fed(batch: &Batch, (taken, runner): (&[Taken], &Runner), rows: Option<&[u32]>, states: &mut [Box<dyn Accumulator>]) -> Result<()> {
    for (state, take) in states.iter_mut().zip(taken) {
        match take {
            // [X3.135-1986 §5.8 GR4b] COUNT(*) counts rows.
            Taken::Count => state.update_rows(rows.map_or(batch.rows, <[u32]>::len))?,
            // [X3.135-1986 §5.8 GR1] nulls taken out.
            Taken::Column(at) => match batch.columns.get(*at) {
                Some(column) => {
                    eliminated(column_nulled(column, rows), runner);
                    fed_column(state, column, rows)?;
                }
                None => state.update_many(&[], rows)?,
            },
            // Its values evaluated a vector at a time (`expr.rs`), or, of a
            // batch of which that gives none, a row at a time.
            Taken::Expr(program) => match lane(program, batch, rows, runner) {
                Some(Fed::Made(column)) => {
                    eliminated(column_nulled(&column, None), runner);
                    fed_column(state, &column, None)?;
                }
                Some(Fed::Const(v)) => {
                    let count = rows.map_or(batch.rows, <[u32]>::len);
                    eliminated(v.is_null() && count > 0, runner);
                    state.update_many(&vec![v.clone(); count], None)?;
                }
                None => fed_by_rows(batch, (&program.expr, &program.inputs, runner), rows, state)?,
            },
        }
    }
    Ok(())
}

/// Feeds `state` the values of `column` at `rows`, or all, a vector at a
/// time, by its kind; a vector of nulls none.
fn fed_column(state: &mut Box<dyn Accumulator>, column: &Column, rows: Option<&[u32]>) -> Result<()> {
    match column {
        Column::Ints(ints) => state.update_ints(ints, rows),
        Column::Floats(floats) => state.update_floats(floats, rows),
        Column::Values(values) => state.update_many(values, rows),
        Column::Texts(texts) => state.update_texts(texts, rows),
        Column::Datetimes(_) | Column::Bools(_) | Column::Late(_) => state.update_many(column.values(), rows),
        // Every value null, none taken.
        Column::Nulls(_) => Ok(()),
    }
}

/// Feeds `state` the values of `e` of the rows `rows` of `batch`, or all,
/// each evaluated as a row's is, its columns the vectors at `inputs`; the
/// warnings raised heard by `runner`.
fn fed_by_rows(
    batch: &Batch,
    (e, inputs, runner): (&Expr, &[usize], &Runner),
    rows: Option<&[u32]>,
    state: &mut Box<dyn Accumulator>,
) -> Result<()> {
    let mut scratch = Vec::new();
    let value = |at: usize, r: usize| batch.value(at, r).unwrap_or(&Value::Null);
    let mut values = Vec::with_capacity(rows.map_or(batch.rows, <[u32]>::len));
    for r in (0..rows.map_or(batch.rows, <[u32]>::len)).map(|n| crate::storage::sel::nth(rows, n)) {
        values.push(evaluated((e, inputs), (&mut scratch, runner), |at, slot| slot.clone_from(value(at, r)))?);
    }
    eliminated(values_nulled(&values, None), runner);
    state.update_many(&values, None)
}

/// The values of `program` of the rows `rows` of `batch`, or all: a
/// constant once, else a vector at a time; none of a batch of which that
/// gives none.
fn lane<'b>(program: &'b Program, batch: &Batch, rows: Option<&[u32]>, runner: &dyn super::Subqueries) -> Option<Fed<'b>> {
    if let Some(v) = program.constant() {
        return Some(Fed::Const(v));
    }
    program.vectored().then(|| program.column(batch, rows, runner)).flatten().map(Fed::Made)
}

/// The values of each of `taken` that is an expression of the rows `rows`
/// of `batch`, or all, evaluated a vector at a time, where they are
/// (`expr.rs`). No statement is at hand: a call that needs one, to keep a
/// large object it makes, gives none, and so does one that would raise a
/// warning, and its rows are evaluated one by one in the statement.
fn lanes<'b>(taken: &'b [Taken], batch: &'b Batch, rows: Option<&'b [u32]>) -> Vec<Option<Fed<'b>>> {
    let alone = super::NoSubqueries;
    taken.iter().map(|t| if let Taken::Expr(program) = t { lane(program, batch, rows, &alone) } else { None }).collect()
}

/// An expression's values of a batch, evaluated a vector at a time: a
/// vector made, or a constant.
enum Fed<'b> {
    Made(Column),
    Const(&'b Value),
}

// Unit tests: which aggregations are read by batches (RULES.md R-TEST-1,
// R-TEST-8).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::tests::state;
    use crate::value::DataType;
    use crate::value::collation::{CASE_INSENSITIVE, CollationRef, builtin, default_pad};

    /// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.13 GR2a] [DESIGN] an
    /// aggregation of a scan is read by batches unless it groups by a
    /// collation or makes groups before any row, which batches do not.
    ///
    /// Decision `!collations.is_empty() || !seeds.is_empty()`: neither
    /// (batches) against seeds alone (rows) shows the seeds' condition;
    /// collations alone (rows) shows the collations'.
    #[test]
    fn seeded_aggregations_are_not_batched() {
        let st = state();
        let table = crate::catalog::QualifiedName { auth: "A".into(), ident: "E".into() };
        let count = st.functions.aggregate("COUNT").unwrap();
        let agg = AggCall {
            func: count,
            arg: None,
            distinct: false,
            result: DataType::Boolean,
            filter: None,
            distribution: None,
            label: "COUNT(*)".into(),
        };
        let scan = || Plan::SeqScan {
            table: table.clone(),
            columns: vec![],
            rows: 0,
            cost: 0.0_f64,
            order: vec![],
            backward: false,
            most: None,
            located: false,
        };
        let aggregate = |seeds: Vec<Vec<Value>>, collations: Vec<Option<CollationRef>>| Plan::Aggregate {
            groups: Vec::new(),
            collations,
            aggs: vec![agg.clone()],
            seeds,
            detail: String::new(),
            input: Box::new(scan()),
            est: 0.0_f64,
        };
        assert!(aggregable(&aggregate(Vec::new(), Vec::new()), &st));
        assert!(!aggregable(&aggregate(vec![Vec::new()], Vec::new()), &st));
        let case_insensitive = builtin().into_iter().find(|c| c.name() == CASE_INSENSITIVE).unwrap();
        let collated = CollationRef { collation: case_insensitive, pad: default_pad(CASE_INSENSITIVE) };
        assert!(!aggregable(&aggregate(Vec::new(), vec![Some(collated)]), &st));
    }
}
