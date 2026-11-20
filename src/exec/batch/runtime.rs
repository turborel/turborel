//! \[DESIGN\] (RULES.md D369) Where a hash join's runtime filters judge
//! the rows of its left operand. A join told its filters to its left
//! operand's stream, each of a column of its rows; each operator takes
//! them down to its scan through those that pass a column on as it is and
//! raise no error: a filter whose condition cannot raise and reads no
//! subquery, a projection of columns, the column it passes on mapped to
//! the one it is of, and the left operand of an inner hash join, of its
//! columns. Any other operator, as an aggregation, an outer join, a LIMIT
//! or one that can raise, judges them of the batches it gives, right
//! above it, as a row below it a filter dropped could have raised an
//! error row by row (RULES.md D15). A scan in no order is told a filter's
//! range as conjuncts (`stream::scan_of`), and judges its rows by it right
//! as it gives each batch, before any column it gives late is decoded.

use std::sync::Arc;

use super::stream::{self, Batches};
use crate::db::State;
use crate::exec::table::RuntimeFilter;
use crate::logical::Expr;
use crate::physical::Plan;
use crate::storage::Sel;

/// Whether hash joins make runtime filters, as EXECUTOR.RUNTIME_FILTERS
/// says.
pub(super) fn wanted(db: &State) -> bool {
    if let crate::value::Value::Char(s) = db.settings.get(crate::settings::EXECUTOR_RUNTIME_FILTERS) { s != "OFF" } else { true }
}

/// A runtime filter told a stream: of the column of its rows' it judges.
#[derive(Debug, Clone)]
pub(in crate::exec) struct Told {
    pub(in crate::exec) filter: Arc<RuntimeFilter>,
    pub(in crate::exec) column: usize,
}

/// The column of the rows of `plan`'s input a filter of its column
/// `column` judges, if its operator takes the filter down to it, as the
/// module's documentation says; of a scan, that column of its table.
fn passed(plan: &Plan, column: usize, db: &State) -> Option<usize> {
    match plan {
        Plan::SeqScan { order, .. } => order.is_empty().then_some(column),
        Plan::Filter { cond, .. } => (!cond.can_raise() && !cond.has_subquery()).then_some(column),
        Plan::Project { exprs, .. } => moved(exprs, column),
        Plan::HashJoin { left, .. } => stream::layout(left, db).filter(|&(_, width)| column < width).map(|_| column),
        Plan::Nearest(_)
        | Plan::IndexScan { .. }
        | Plan::IndexJoin { .. }
        | Plan::MergeJoin(_)
        | Plan::Product { .. }
        | Plan::OuterJoin { .. }
        | Plan::Aggregate { .. }
        | Plan::Append { .. }
        | Plan::MergeAppend { .. }
        | Plan::SetOp { .. }
        | Plan::Values { .. }
        | Plan::System { .. }
        | Plan::Sort { .. }
        | Plan::Limit { .. }
        | Plan::Distinct { .. }
        | Plan::Recursive { .. }
        | Plan::Unnest(..)
        | Plan::Window { .. }
        | Plan::Polymorphic(..)
        | Plan::Recognize { .. }
        | Plan::WorkTable { .. }
        | Plan::Transition { .. }
        | Plan::Parallel(..) => None,
    }
}

/// The column of a projection's input that its column `column` is, of
/// `exprs`, if it is one moved as it is.
fn moved(exprs: &[Expr], column: usize) -> Option<usize> {
    if let Some(Expr::Column(c)) = exprs.get(column) { Some(*c) } else { None }
}

/// Of `told`, the filters a projection by `exprs` takes down to its input,
/// each of the column it moves: those of columns moved as they are.
pub(super) fn projected(exprs: &[Expr], told: &[Told]) -> Vec<Told> {
    told.iter().filter_map(|t| moved(exprs, t.column).map(|column| Told { filter: Arc::clone(&t.filter), column })).collect()
}

/// Of `told`, the filters `plan`'s operator takes down to its input, each
/// of the column of its rows it judges there (`passed`), and those judged
/// of the batches it gives.
pub(super) fn taken(plan: &Plan, told: &[Told], db: &State) -> (Vec<Told>, Vec<Told>) {
    let (mut below, mut here) = (Vec::new(), Vec::new());
    for t in told {
        match passed(plan, t.column, db) {
            Some(column) => below.push(Told { filter: Arc::clone(&t.filter), column }),
            None => here.push(t.clone()),
        }
    }
    (below, here)
}

/// Whether a filter of column `column` of `plan`'s rows would reach a scan
/// in no order, each operator on the way taking it down (`passed`).
pub(super) fn reaches(plan: &Plan, column: usize, db: &State) -> bool {
    let (mut at, mut column) = (plan, column);
    while let Some(next) = passed(at, column, db) {
        column = next;
        if matches!(at, Plan::SeqScan { .. }) {
            return true;
        }
        at = if let Plan::Filter { input, .. } | Plan::Project { input, .. } = at {
            input
        } else if let Plan::HashJoin { left, .. } = at {
            left
        } else {
            return false;
        };
    }
    false
}

/// `batches`, each of whose vectors holds the column of its rows at its
/// place in `columns`, the rows each of `told` keeps of each batch kept,
/// judged while it judges (`RuntimeFilter::judging`).
pub(super) fn applied(batches: Batches, told: Vec<Told>, columns: &[usize], db: &Arc<State>) -> Batches {
    let placed: Vec<(Arc<RuntimeFilter>, usize)> =
        told.into_iter().filter_map(|t| columns.iter().position(|&c| c == t.column).map(|at| (t.filter, at))).collect();
    if placed.is_empty() {
        return batches;
    }
    let db = Arc::clone(db);
    Box::new(batches.map(move |kept| {
        let (batch, mut rows) = kept?;
        for (filter, at) in &placed {
            if filter.judging() {
                rows = Some(Sel::of(filter.kept((&batch, *at), rows.as_deref(), db.functions.kernels())?));
            }
        }
        Ok((batch, rows))
    }))
}
