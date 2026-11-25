//! \[DESIGN\] Lineage (RULES.md D370): the base table column a plan's
//! column is, through every node that passes its input's columns on, so a
//! condition, an equality or a grouping above a join, a grouping or a
//! derived table is estimated by that table's statistics.
//!
//! A filter, a sort, a limit and DISTINCT give their input's columns; a
//! window gives them first, then its functions'; a projection of a column
//! reference gives that column; a product and an outer join give their
//! left operand's then their right's; a grouping gives its grouping
//! columns first, each its input's. A column of the null-supplying side of
//! an outer join keeps its table's distinct values: the nulls added are
//! one value more, which no distinct count of a key needs. Any other
//! column, an expression's, a set function's or a set operation's, has no
//! table.
//!
//! \[DESIGN\] (RULES.md D1082) A projection's expression that is
//! deterministic is a function of the columns it reads, so it takes no
//! more values than those columns take together (`computed_of`), which
//! bounds the groups of a grouping by it.

use super::MAX_ESTIMATE_DEPTH;
use crate::catalog::QualifiedName;
use crate::logical::{Expr, LogicalPlan, expr_columns};
use crate::optimizer::unnesting::expr_pure;

/// A column's table: the scan reading it, its table, and the column's
/// place among the table's columns.
#[derive(Clone, Copy)]
pub(super) struct Origin<'p> {
    pub(super) scan: &'p LogicalPlan,
    pub(super) table: &'p QualifiedName,
    pub(super) column: usize,
}

/// The table `plan`'s column `column` is a column of, if it is one.
pub(super) fn origin(plan: &LogicalPlan, column: usize) -> Option<Origin<'_>> {
    let (mut at, mut column) = (plan, column);
    for _ in 0..MAX_ESTIMATE_DEPTH {
        let (next, place) = step(at, column)?;
        if let LogicalPlan::Scan { table, .. } = next {
            return Some(Origin { scan: next, table, column: place });
        }
        (at, column) = (next, place);
    }
    None
}

/// \[DESIGN\] (RULES.md D1082) The node whose columns `plan`'s column
/// `column` is computed of, and those columns, ascending, if it is an
/// expression of a projection, reached through the nodes that pass
/// columns on, that gives the same value of the same columns each time
/// (`expr_pure`): none of a table's column, nor of an expression that is
/// not deterministic, which may take a value of its own for every row.
pub(super) fn computed_of(plan: &LogicalPlan, column: usize) -> Option<(&LogicalPlan, Vec<usize>)> {
    let (mut at, mut place) = (plan, column);
    for _ in 0..MAX_ESTIMATE_DEPTH {
        if let LogicalPlan::Project { exprs, input } = at
            && let Some(e) = exprs.get(place).filter(|e| !matches!(e, Expr::Column(_)))
        {
            let mut read = Vec::new();
            expr_columns(e, &mut read);
            read.sort_unstable();
            read.dedup();
            return expr_pure(e).then_some((&**input, read));
        }
        if matches!(at, LogicalPlan::Scan { .. }) {
            return None;
        }
        (at, place) = step(at, place)?;
    }
    None
}

/// The node `plan`'s column `column` comes from and its place there: the
/// node itself, of a table's scan; its input, of a node passing columns on.
fn step(plan: &LogicalPlan, column: usize) -> Option<(&LogicalPlan, usize)> {
    match plan {
        LogicalPlan::Scan { .. } => Some((plan, column)),
        LogicalPlan::Filter { input, .. }
        | LogicalPlan::Sort { input, .. }
        | LogicalPlan::Limit { input, .. }
        | LogicalPlan::Distinct { input }
        | LogicalPlan::SemiJoin { left: input, .. } => Some((&**input, column)),
        LogicalPlan::MarkJoin(m) => (column < m.left.width()).then_some((&m.left, column)),
        LogicalPlan::Window(w) => (column < w.input.width()).then_some((&w.input, column)),
        LogicalPlan::Project { exprs, input } => match exprs.get(column) {
            Some(Expr::Column(c)) => Some((&**input, *c)),
            Some(_) | None => None,
        },
        LogicalPlan::Product { left, right } | LogicalPlan::OuterJoin { left, right, .. } => {
            Some(column.checked_sub(left.width()).map_or((&**left, column), |of_right| (&**right, of_right)))
        }
        LogicalPlan::Aggregate { groups, input, .. } => Some((&**input, *groups.get(column)?)),
        LogicalPlan::SetOp { .. }
        | LogicalPlan::System { .. }
        | LogicalPlan::Values { .. }
        | LogicalPlan::Recursive { .. }
        | LogicalPlan::WorkTable { .. }
        | LogicalPlan::Transition { .. }
        | LogicalPlan::Unnest(..)
        | LogicalPlan::Recognize(..)
        | LogicalPlan::Polymorphic(..) => None,
    }
}

/// The columns a condition is over: one plan's, or a pair's, an outer
/// join's operands', the left's first.
#[derive(Clone, Copy)]
pub(super) enum Over<'p> {
    One(&'p LogicalPlan),
    Pair(&'p LogicalPlan, &'p LogicalPlan),
}

impl<'p> Over<'p> {
    /// The plan column `column` is of, and its place there.
    pub(super) fn side(self, column: usize) -> (&'p LogicalPlan, usize) {
        match self {
            Over::One(plan) => (plan, column),
            Over::Pair(left, right) => column.checked_sub(left.width()).map_or((left, column), |of_right| (right, of_right)),
        }
    }

    /// The table column `column` is a column of, if it is one.
    pub(super) fn origin(self, column: usize) -> Option<Origin<'p>> {
        let (plan, at) = self.side(column);
        origin(plan, at)
    }
}

/// The one scan every column of `columns` of `over` comes from, the same
/// node, not only the same table, as a table joined with itself is two
/// relations; its table, and each column's place among the table's.
pub(super) fn one_table<'p>(over: Over<'p>, columns: &[usize]) -> Option<(&'p QualifiedName, Vec<(usize, usize)>)> {
    let mut first: Option<Origin<'p>> = None;
    let mut places = Vec::with_capacity(columns.len());
    for &c in columns {
        let found = over.origin(c)?;
        if first.is_some_and(|known| !std::ptr::eq(known.scan, found.scan)) {
            return None;
        }
        first = Some(found);
        places.push((c, found.column));
    }
    Some((first?.table, places))
}
