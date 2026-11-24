//! \[DESIGN\] (RULES.md D1062, D15) The declared types of the columns a
//! runtime filter passes on its way down, and what they prove cannot
//! raise. A filter may go below a projection or into a grouping only if
//! nothing there can raise an error, as a row it drops is then never
//! evaluated (`runtime.rs`, `told.rs`). `Expr::can_raise` takes every
//! arithmetic operator as able to, and a grouping's SUM and AVG as able
//! to overflow. By the types of the columns they read most cannot
//! (`logical/raises.rs`, RULES.md D373, D1060), so the projection over an
//! unnested subquery's grouping, and the grouping, take the filter.

use super::stream;
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::functions::ColumnarKind;
use crate::logical::raises::{set_function_type, total_fits};
use crate::logical::{AggCall, Expr, MAX_JUDGED_DEPTH, can_raise_typed};
use crate::physical::Plan;
use crate::value::DataType;

/// The declared type of column `column` of the rows of `plan`, if known:
/// of a table's column, through the operators that pass a column on as
/// it is, an inner hash join's operands, and a grouping, whose set
/// functions bound their values (`logical::set_function_type`).
pub(super) fn column_type(plan: &Plan, column: usize, db: &State) -> Option<DataType> {
    typed(plan, column, db, 0)
}

/// `column_type`, `depth` operators down; past `MAX_JUDGED_DEPTH` not
/// known (RULES.md R-CF-6).
fn typed(plan: &Plan, column: usize, db: &State, depth: usize) -> Option<DataType> {
    if depth > MAX_JUDGED_DEPTH {
        return None;
    }
    let deeper = depth.saturating_add(1);
    if let Some(input) = passing(plan) {
        return typed(input, column, db, deeper);
    }
    if let Some(table) = scanned(plan) {
        return db.catalog.table(table)?.columns.get(column).map(|c| c.data_type.clone());
    }
    if let Plan::Project { exprs, input, .. } = plan {
        let Expr::Column(c) = exprs.get(column)? else { return None };
        return typed(input, *c, db, deeper);
    }
    if let Plan::HashJoin { left, right, .. } = plan {
        let (_, width) = stream::layout(left, db)?;
        return match column.checked_sub(width) {
            Some(of_right) => typed(right, of_right, db, deeper),
            None => typed(left, column, db, deeper),
        };
    }
    let Plan::Aggregate { groups, aggs, seeds, input, .. } = plan else { return None };
    if !seeds.is_empty() {
        return None;
    }
    match column.checked_sub(groups.len()) {
        Some(n) => set_function_type(aggs.get(n)?, &|c| typed(input, c, db, deeper)),
        None => typed(input, *groups.get(column)?, db, deeper),
    }
}

/// The table `plan` reads, if it is a scan whose rows are the table's.
fn scanned(plan: &Plan) -> Option<&QualifiedName> {
    if let Plan::SeqScan { table, located: false, .. } | Plan::IndexScan { table, located: false, .. } = plan { Some(table) } else { None }
}

/// The input of `plan`, if its operator passes each column of its input's
/// rows on as it is, in its place, and adds no row.
fn passing(plan: &Plan) -> Option<&Plan> {
    match plan {
        Plan::Filter { input, .. } | Plan::Sort { input, .. } => Some(input),
        Plan::Parallel(p) => Some(&p.input),
        Plan::Counted(c) => Some(&c.input),
        Plan::SeqScan { .. }
        | Plan::IndexScan { .. }
        | Plan::Project { .. }
        | Plan::HashJoin { .. }
        | Plan::Aggregate { .. }
        | Plan::Nearest(_)
        | Plan::Product { .. }
        | Plan::IndexJoin { .. }
        | Plan::OuterJoin { .. }
        | Plan::MergeJoin(_)
        | Plan::RangeJoin(_)
        | Plan::SemiJoin(_)
        | Plan::MarkJoin(_)
        | Plan::Distinct { .. }
        | Plan::SetOp { .. }
        | Plan::System { .. }
        | Plan::Values { .. }
        | Plan::Append { .. }
        | Plan::MergeAppend { .. }
        | Plan::Limit { .. }
        | Plan::Recursive { .. }
        | Plan::WorkTable { .. }
        | Plan::Transition { .. }
        | Plan::Unnest(..)
        | Plan::Window { .. }
        | Plan::Recognize { .. }
        | Plan::Polymorphic(..)
        | Plan::Transfer(_) => None,
    }
}

/// Whether column `column` of the rows of `plan` is known to hold no
/// null: a table's column declared so, through the operators that pass a
/// column on as it is and add no row of nulls.
pub(super) fn never_null(plan: &Plan, column: usize, db: &State) -> bool {
    filled(plan, column, db, 0)
}

/// `never_null`, `depth` operators down; past `MAX_JUDGED_DEPTH` not
/// known (RULES.md R-CF-6).
fn filled(plan: &Plan, column: usize, db: &State, depth: usize) -> bool {
    if depth > MAX_JUDGED_DEPTH {
        return false;
    }
    let deeper = depth.saturating_add(1);
    if let Some(input) = passing(plan) {
        return filled(input, column, db, deeper);
    }
    if let Some(table) = scanned(plan) {
        return db.catalog.table(table).and_then(|t| t.columns.get(column).map(|c| c.not_null)).unwrap_or(false);
    }
    if let Plan::Project { exprs, input, .. } = plan {
        let Some(Expr::Column(c)) = exprs.get(column) else { return false };
        return filled(input, *c, db, deeper);
    }
    let Plan::HashJoin { left, right, .. } = plan else { return false };
    match stream::layout(left, db).map(|(_, width)| column.checked_sub(width)) {
        Some(Some(of_right)) => filled(right, of_right, db, deeper),
        Some(None) => filled(left, column, db, deeper),
        None => false,
    }
}

/// Whether evaluating any of `exprs`, over the rows of `input`, can raise
/// an error: as `Expr::can_raise` judges it, unless the declared types of
/// the columns it reads prove it cannot.
pub(super) fn exprs_raise(exprs: &[Expr], input: &Plan, db: &State) -> bool {
    exprs.iter().any(|e| e.can_raise() && can_raise_typed(e, &|c| column_type(input, c, db)))
}

/// Whether the set function `a`, over the rows of `input`, raises nothing
/// of its own whatever rows it is given: COUNT, MIN and MAX, and SUM and
/// AVG of exact values whose sum the declared types keep within an exact
/// number's digits (RULES.md D1060).
pub(super) fn quiet(a: &AggCall, input: &Plan, db: &State) -> bool {
    match a.func.0.columnar().map(|c| c.0) {
        Some(ColumnarKind::Count | ColumnarKind::Min | ColumnarKind::Max) => true,
        Some(ColumnarKind::Sum | ColumnarKind::Avg) => total_fits(a, &|c| column_type(input, c, db)),
        None => false,
    }
}
