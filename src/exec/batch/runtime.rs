//! \[DESIGN\] (RULES.md D369) Where a hash join's runtime filters judge
//! the rows of its left operand. A join told its filters to its left
//! operand's stream, each of a column of its rows; each operator takes
//! them down to its scan through those that pass a column on as it is and
//! raise no error: a filter whose condition cannot raise and reads no
//! subquery, a projection of columns, the column it passes on mapped to
//! the one it is of, if none of its expressions can raise, and the left
//! operand of an inner hash join, of its columns. A part hoisted under its
//! guard (RULES.md D373) is such a projection over a product, so a
//! filter never reaches the operand the guard asks has a row. Any other
//! operator, as an aggregation, an outer join, a LIMIT, one that can
//! raise, or one an analyzed plan counts (RULES.md D378), judges them of
//! the batches it gives, right above it, as a row below it a filter
//! dropped could have raised an error row by row (RULES.md D15). A scan
//! in no order is told a filter's range as conjuncts (`stream::scan_of`),
//! and judges its rows by it right as it gives each batch, before any
//! column it gives late is decoded.

use std::sync::Arc;

use super::stream::{self, Batches};
use crate::db::State;
use crate::exec::table::RuntimeFilter;
use crate::logical::Expr;
use crate::physical::Plan;
use crate::storage::Sel;

/// Whether hash joins make runtime filters, as EXECUTOR.RUNTIME_FILTERS
/// says.
pub(in crate::exec) fn wanted(db: &State) -> bool {
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
        Plan::Project { exprs, .. } => moved(exprs, column).filter(|_| !exprs.iter().any(Expr::can_raise)),
        Plan::HashJoin { left, .. } => stream::layout(left, db).filter(|&(_, width)| column < width).map(|_| column),
        Plan::Nearest(_)
        | Plan::IndexScan { .. }
        | Plan::IndexJoin { .. }
        | Plan::MergeJoin(_)
        | Plan::RangeJoin(_)
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
        | Plan::Parallel(..)
        | Plan::Counted(_) => None,
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
pub(in crate::exec) fn taken(plan: &Plan, told: &[Told], db: &State) -> (Vec<Told>, Vec<Told>) {
    let (mut below, mut here) = (Vec::new(), Vec::new());
    for t in told {
        match passed(plan, t.column, db) {
            Some(column) => below.push(Told { filter: Arc::clone(&t.filter), column }),
            None => here.push(t.clone()),
        }
    }
    (below, here)
}

/// Of `told`, the filters `plan`'s operator takes down to its input, as
/// told it, of its rows' columns, and those judged of the batches it
/// gives (`taken`).
pub(super) fn parted(plan: &Plan, told: &[Told], db: &State) -> (Vec<Told>, Vec<Told>) {
    told.iter().cloned().partition(|t| passed(plan, t.column, db).is_some())
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

/// \[DESIGN\] (RULES.md D389) What a filter judges, its range and its
/// Bloom filter if it `blooms`, of a scan if `at_scan`, or above an
/// operator: none if neither. A scan judges the range among its
/// conjuncts, so the filter does not again.
fn asked((blooms, at_scan): (bool, bool)) -> Option<(bool, bool)> {
    (blooms || !at_scan).then_some((!at_scan, blooms))
}

/// `batches`, each of whose vectors holds the column of its rows at its
/// place in `columns`, the rows each of `told` keeps of each batch kept,
/// judged while it judges (`RuntimeFilter::judging`), so a filter that
/// keeps most rows stops costing anything (RULES.md D389). Of a scan,
/// `at_scan`, its ranges are the scan's own conjuncts, and its Bloom
/// filter alone is judged, right as the scan gives each batch, before
/// any column given late is decoded; above another operator, both.
pub(in crate::exec) fn applied(batches: Batches, told: Vec<Told>, columns: &[usize], (db, at_scan): (&Arc<State>, bool)) -> Batches {
    let placed: Vec<(Arc<RuntimeFilter>, usize, (bool, bool))> = told
        .into_iter()
        .filter_map(|t| {
            let at = columns.iter().position(|&c| c == t.column)?;
            asked((t.filter.blooms(), at_scan)).map(|asked| (t.filter, at, asked))
        })
        .collect();
    if placed.is_empty() {
        return batches;
    }
    let db = db.clone();
    Box::new(batches.map(move |kept| {
        let (batch, mut rows) = kept?;
        for (filter, at, asked) in &placed {
            if filter.judging() {
                rows = Some(Sel::of(filter.kept((&batch, *at), (rows.as_deref(), *asked), db.functions.kernels())?));
            }
        }
        Ok((batch, rows))
    }))
}

// Unit tests: what a filter judges where (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::asked;

    /// Verifies: [DESIGN] RULES.md D389 (MC/DC): of a scan, the range,
    /// among its conjuncts, is not judged again, and the Bloom filter is
    /// if there is one; above an operator, the range and the Bloom filter
    /// if any; of a scan and no Bloom filter, nothing.
    #[test]
    fn a_scan_judges_its_range_once() {
        assert_eq!(asked((true, true)), Some((false, true)), "a scan");
        assert_eq!(asked((true, false)), Some((true, true)), "above an operator");
        assert_eq!(asked((false, false)), Some((true, false)), "above an operator, no Bloom filter");
        assert_eq!(asked((false, true)), None, "a scan, no Bloom filter");
    }
}
