//! \[DESIGN\] A projection of tables read a batch at a time (RULES.md D31):
//! of a subtree that gives batches (`stream.rs`), every table of which
//! reads by batch (`TableStorage::reads_by_batch`), whose expressions read
//! only columns its batches hold, no subquery nor outer reference among
//! them, each row given made of the batch's vectors, no row of the table's
//! width made for it, a batch read ahead of the rows given. A column or a
//! constant is taken as it is; any other expression is compiled once and
//! evaluated a vector at a time (`expr.rs`, RULES.md D362), or, of a batch
//! of which that gives none, as a row's is (`exec/eval.rs`), of a row of
//! only the values it reads. Rows come as the subtree gives batches, in no
//! particular order but its own; a projection of anything else is made row
//! by row (`exec.rs`). A projection
//! read by a stream's reader, as DISTINCT, a grouping, a join or UNION ALL
//! is, is a stream itself, its batches its expressions' vectors
//! (`computed`). A call of a function that gives many rows' values at
//! once (`ScalarFunction::batches`), as EMBED of a model does (vector
//! design R12.3), is called once for the rows of a batch; if that fails,
//! a row at a time, so its error comes at its row.

use std::sync::Arc;

use super::expr::{Program, gather};
use super::stream::{self, Batches};
use crate::db::State;
use crate::error::{Result, SqlError};
use crate::exec::Db;
use crate::exec::{RowIter, Runner, eval};
use crate::logical::{Expr, expr_columns, map_expr_columns};
use crate::physical::Plan;
use crate::storage::{Batch, Column, Row};
use crate::txn::TxId;
use crate::value::Value;

/// What an expression of the projection gives: the value of the column at
/// this place of a batch, a constant, or an expression of a row of the
/// values at some places, its columns those of the row, compiled for
/// vectors once (`expr.rs`).
#[derive(Clone)]
pub(in crate::exec) enum Place {
    Column(usize),
    Const(Value),
    Expr(Arc<Program>),
}

/// Whether `e` reads nothing but its row: no subquery, routine invoked
/// nor outer reference, in it or in the conditions of a CASE in it.
fn plain(e: &Expr) -> bool {
    let own = match e {
        // A value drawn for each row, in the row's order [ISO/IEC 9075-2:2023 §6.14 GR1].
        Expr::Subquery(..) | Expr::Invoked(_) | Expr::Outer(..) | Expr::Dynamic(_) | Expr::NextValue(_) => false,
        Expr::Case(whens, _) => whens.iter().all(|(c, _)| c.subqueries().is_empty()),
        Expr::Truth(c) => c.subqueries().is_empty(),
        Expr::Column(_)
        | Expr::Const(_)
        | Expr::Neg(_)
        | Expr::Arith(..)
        | Expr::Cast(..)
        | Expr::SortKey(..)
        | Expr::Domain(..)
        | Expr::Warned(..) => true,
        // One that reads its statement is evaluated a row at a time (D87).
        Expr::Call(function, args) => !function.0.uses_context(args.len()),
    };
    let mut all = own;
    e.for_each_child(&mut |child| all = all && plain(child));
    all
}

/// What `e`, of a scan of `columns`, gives of a batch, if it reads only
/// columns the scan reads. A column alone the scan does not read is null,
/// as a row's scan gives it (`TableStorage::scan` decodes only the columns
/// asked for): a projection restoring the order of a join's columns names
/// every column, those the query does not read too.
pub(super) fn place_of(e: &Expr, columns: &[usize]) -> Option<Place> {
    let at = |c: &usize| columns.iter().position(|x| x == c);
    match e {
        Expr::Column(c) => Some(at(c).map_or(Place::Const(Value::Null), Place::Column)),
        Expr::Const(v) => Some(Place::Const(v.clone())),
        Expr::Neg(_)
        | Expr::Arith(..)
        | Expr::Call(..)
        | Expr::Outer(..)
        | Expr::Dynamic(_)
        | Expr::Case(..)
        | Expr::Cast(..)
        | Expr::SortKey(..)
        | Expr::Domain(..)
        | Expr::Warned(..)
        | Expr::Subquery(..)
        | Expr::Truth(_)
        | Expr::NextValue(_)
        | Expr::Invoked(_) => {
            if !plain(e) {
                return None;
            }
            let mut read = Vec::new();
            expr_columns(e, &mut read);
            read.sort_unstable();
            read.dedup();
            let inputs = read.iter().map(at).collect::<Option<Vec<usize>>>()?;
            let own = map_expr_columns(e.clone(), &|c| read.iter().position(|&r| r == c).unwrap_or(usize::MAX));
            Some(Place::Expr(Arc::new(Program::of(own, inputs))))
        }
    }
}

/// A projection's rows, made of the batches as they are read.
struct Projected {
    batches: Batches,
    places: Vec<Place>,
    runner: Runner,
    batch: Batch,
    /// The rows of the batch the filter kept, and the next to give.
    kept: Vec<usize>,
    next: usize,
    /// The row an expression is evaluated of.
    scratch: Row,
    /// Of each place a call given many rows at once, its values of the
    /// rows kept of the batch.
    ahead: Vec<Option<Vec<Value>>>,
}

/// Whether `plan` is a projection read by batches: of a subtree that gives
/// them (`stream::layout`), every table of which reads by batch, its every
/// expression reading only columns they hold.
pub(in crate::exec) fn projectable(plan: &Plan, db: &State) -> bool {
    let Plan::Project { exprs, input, .. } = plan else { return false };
    let Some((columns, _)) = stream::layout(input, db) else { return false };
    // A table that makes each row of a batch gives rows as fast read one at
    // a time, without reading ahead.
    stream::reads_by_batch(input, db) && exprs.iter().all(|e| place_of(e, &columns).is_some())
}

/// The rows of the projection `plan`, one read by batches (`projectable`).
pub(in crate::exec) fn projected(plan: Plan, db: &Db, tx: TxId) -> RowIter {
    let Plan::Project { exprs, input, .. } = plan else { return crate::exec::failed(crate::exec::broken("a projection is not one")) };
    let stream = match stream::open(*input, db, tx) {
        Ok(stream) => stream,
        Err(e) => return crate::exec::failed(e),
    };
    let Some(places) = exprs.iter().map(|e| place_of(e, &stream.columns)).collect::<Option<Vec<_>>>() else {
        return crate::exec::failed(crate::exec::broken("a projection not read by batches"));
    };
    Box::new(Projected {
        batches: stream.batches,
        places,
        runner: Runner::new(db, tx),
        batch: Batch::default(),
        kept: Vec::new(),
        next: 0,
        scratch: Vec::new(),
        ahead: Vec::new(),
    })
}

impl Projected {
    /// The next row given, if any is left.
    fn step(&mut self) -> Result<Option<Row>> {
        while self.next >= self.kept.len() {
            let Some((batch, rows)) = self.batches.next().transpose()? else { return Ok(None) };
            self.ahead = self.places.iter().map(|place| ahead(place, (&batch, rows.as_deref()), &self.runner)).collect();
            self.kept = crate::storage::sel::places_of(rows, batch.rows);
            (self.batch, self.next) = (batch, 0);
        }
        let r = self.kept.get(self.next).copied().unwrap_or_default();
        self.next = self.next.saturating_add(1);
        // [DESIGN] (RULES.md D367) A cell of the row alone made a value, its
        // vector's other rows not made values.
        let value = |at: &usize| self.batch.cell(*at, r).to_value();
        let mut row = Vec::with_capacity(self.places.len());
        for (n, place) in self.places.iter().enumerate() {
            if let Some(v) = self.ahead.get(n).and_then(Option::as_ref).and_then(|values| values.get(self.next.saturating_sub(1))) {
                row.push(v.clone());
                continue;
            }
            row.push(match place {
                Place::Column(at) => value(at),
                Place::Const(v) => v.clone(),
                // Its values in a row kept, each written over.
                Place::Expr(program) => {
                    self.scratch.resize(program.inputs.len(), Value::Null);
                    for (slot, at) in self.scratch.iter_mut().zip(&program.inputs) {
                        *slot = value(at);
                    }
                    eval(&program.expr, &self.scratch, &self.runner)?
                }
            });
        }
        Ok(Some(row))
    }
}

/// The values of `place` of the rows `rows` of `batch`, or all, made
/// ahead of the rows given: of a call given many rows at once, by one
/// call; else a vector at a time; none of a column or a constant, or of
/// a batch whose evaluation would raise, whose rows are then evaluated
/// one by one as they are given.
fn ahead(place: &Place, (batch, rows): (&Batch, Option<&[u32]>), runner: &Runner) -> Option<Vec<Value>> {
    let Place::Expr(program) = place else { return None };
    if !program.vectored() && !matches!(&program.expr, Expr::Call(..) | Expr::SortKey(..)) {
        return None;
    }
    expr_vector(program, (batch, rows), runner)?.ok().map(Column::into_values)
}

/// \[DESIGN\] The batches of a projection of a stream's `batches`, its
/// expressions given by `places` of their vectors, as a stream of its own,
/// its columns the expressions': each batch of the rows a filter kept of
/// one of `batches`, each expression evaluated of them only, a vector at a
/// time where it can be (`expr.rs`), else a row at a
/// time, as row by row; its warnings heard by `runner`. A batch of which
/// one raises an error is evaluated again a row at a time, in order, so
/// the rows before its failing one come first, then the error, as a
/// projection row by row gives them (RULES.md D15).
pub(super) fn computed(batches: Batches, places: Vec<Place>, runner: Runner) -> Batches {
    Box::new(batches.flat_map(move |kept| {
        let made = kept.map(|(batch, rows)| {
            if let Ok(made) = by_vectors(&batch, rows.as_deref(), (&places, &runner)) {
                return vec![Ok((made, None))];
            }
            let (before, failed) = by_rows(&batch, rows.as_deref(), (&places, &runner));
            vec![Ok((before, None))].into_iter().chain(failed.map(Err)).collect()
        });
        made.unwrap_or_else(|e| vec![Err(e)])
    }))
}

/// The vectors of `places` of the rows `rows` of `batch`, or all, a batch
/// of them alone: an expression's evaluated a vector at a time
/// (`expr.rs`), its warnings heard by `runner`; an error if one of them
/// gives none, so the batch is evaluated again a row at a time.
fn by_vectors(batch: &Batch, rows: Option<&[u32]>, (places, runner): (&[Place], &Runner)) -> Result<Batch> {
    let count = rows.map_or(batch.rows, <[u32]>::len);
    let mut columns = Vec::with_capacity(places.len());
    for place in places {
        columns.push(match place {
            // Of every row, its vector as it is; of some, gathered, of its kind.
            Place::Column(at) => {
                let column = batch.columns.get(*at).cloned().unwrap_or_else(|| Column::nulls(batch.rows));
                match rows {
                    None => column,
                    Some(rows) => gather(&column, rows),
                }
            }
            Place::Const(v) => super::expr::constant_column(v, count),
            Place::Expr(program) => {
                expr_vector(program, (batch, rows), runner).ok_or_else(|| crate::exec::broken("a batch evaluated a row at a time"))??
            }
        });
    }
    Ok(Batch { columns, rows: count })
}

/// \[DESIGN\] (RULES.md D560) The vector of `e`, over the columns of the
/// rows of `batch`, of all of them, as a window over batches reads its
/// calls' arguments: none if `e` is not evaluated so, or a row's
/// evaluation would raise an error or a warning, so the window computes
/// those rows a row at a time.
pub(in crate::exec) fn vector_of(e: &Expr, batch: &Batch, runner: &Runner) -> Option<Column> {
    let columns: Vec<usize> = (0..batch.columns.len()).collect();
    match place_of(e, &columns)? {
        Place::Column(at) => batch.columns.get(at).cloned(),
        Place::Const(v) => Some(super::expr::constant_column(&v, batch.rows)),
        Place::Expr(program) => expr_vector(&program, (batch, None), runner)?.ok(),
    }
}

/// The vector of `program` of the rows `rows` of `batch`, or all: of a
/// call given many rows at once, by one call, else a vector at a time;
/// none if that gives none.
fn expr_vector(program: &Program, (batch, rows): (&Batch, Option<&[u32]>), runner: &Runner) -> Option<Result<Column>> {
    if matches!(&program.expr, Expr::Call(..) | Expr::SortKey(..)) {
        let given: Vec<usize> = (0..rows.map_or(batch.rows, <[u32]>::len)).map(|n| crate::storage::sel::nth(rows, n)).collect();
        if let Some(values) = batched(program, (batch, &given), runner) {
            return Some(values.map(Column::Values));
        }
    }
    program.column(batch, rows, runner).map(Ok)
}

/// The values of `program` of the rows `given` of `batch`, if it is a
/// call of a function that gives many rows' values at once: its
/// arguments evaluated a row at a time, then the function called once.
fn batched(program: &Program, (batch, given): (&Batch, &[usize]), runner: &Runner) -> Option<Result<Vec<Value>>> {
    let (e, inputs) = (&program.expr, program.inputs.as_slice());
    if let Expr::SortKey(x, collation) = e {
        return keys_batched((x, inputs), (batch, given), (collation, runner));
    }
    let Expr::Call(function, args) = e else { return None };
    if !function.0.batches() {
        return None;
    }
    let mut scratch = Vec::new();
    let mut rows = Vec::with_capacity(given.len());
    for &r in given {
        let row: Result<Vec<Value>> = args.iter().map(|a| evaluated_of((a, inputs), (batch, r), (&mut scratch, runner))).collect();
        match row {
            Ok(row) => rows.push(row),
            Err(error) => return Some(Err(error)),
        }
    }
    Some(function.0.call_many_in(&rows, runner))
}

/// The sort keys of `x` of the rows `given` of `batch`, if `collation`
/// makes many at once, as a plug-in's does in one call (RULES.md D221).
fn keys_batched(
    (x, inputs): (&Expr, &[usize]),
    (batch, given): (&Batch, &[usize]),
    (collation, runner): (&crate::value::collation::CollationRef, &Runner),
) -> Option<Result<Vec<Value>>> {
    if !collation.collation.batches() {
        return None;
    }
    let mut scratch = Vec::new();
    let values: Result<Vec<Value>> = given.iter().map(|&r| evaluated_of((x, inputs), (batch, r), (&mut scratch, runner))).collect();
    Some(values.and_then(|v| collation.sort_keys(&v)))
}

/// The value of `e`, of a row of the values at `inputs` of row `r` of
/// `batch`, made in `scratch`, its warnings heard by `runner`.
fn evaluated_of((e, inputs): (&Expr, &[usize]), (batch, r): (&Batch, usize), (scratch, runner): (&mut Row, &Runner)) -> Result<Value> {
    scratch.resize(inputs.len(), Value::Null);
    for (slot, &at) in scratch.iter_mut().zip(inputs) {
        *slot = batch.cell(at, r).to_value();
    }
    eval(e, scratch, runner)
}

/// The rows `rows` of `batch`, or all, each of the values of `places`,
/// evaluated a row at a time in order, until one raises an error: a batch
/// of those before it, and the error, if one did.
fn by_rows(batch: &Batch, rows: Option<&[u32]>, (places, runner): (&[Place], &Runner)) -> (Batch, Option<SqlError>) {
    let mut out = Batch::empty(places.len());
    let mut scratch = Vec::new();
    for r in (0..rows.map_or(batch.rows, <[u32]>::len)).map(|n| crate::storage::sel::nth(rows, n)) {
        let row: Result<Vec<Value>> = places
            .iter()
            .map(|place| match place {
                Place::Column(at) => Ok(batch.cell(*at, r).to_value()),
                Place::Const(v) => Ok(v.clone()),
                Place::Expr(program) => evaluated_of((&program.expr, &program.inputs), (batch, r), (&mut scratch, runner)),
            })
            .collect();
        match row {
            Ok(values) => {
                for (vector, v) in out.columns.iter_mut().zip(values) {
                    vector.values_mut().push(v);
                }
                out.rows = out.rows.saturating_add(1);
            }
            Err(e) => return (out, Some(e)),
        }
    }
    (out, None)
}

impl Iterator for Projected {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Self::Item> {
        self.step().transpose()
    }
}
