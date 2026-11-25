//! \[DESIGN\] (RULES.md D1077) The antijoin of a NOT IN over columns that
//! hold no null, keyed. NOT IN is the antijoin on `(x = y) IS NOT FALSE`
//! (`semi.rs`), which a null on either side satisfies [ISO/IEC 9075:1992
//! §8.7 GR2]. That condition is no equality of columns, so its join is a
//! nested loop, every outer row read against every row of the subquery.
//! Of two columns declared to hold no null, `x = y` is never unknown, so
//! `(x = y) IS NOT FALSE` is `x = y`, and written so the join is hashed
//! on it. A column that may hold a null keeps the condition as it was.

use crate::catalog::Catalog;
use crate::logical::{Cond, Expr, LogicalPlan, MAX_JUDGED_DEPTH, Truth, and_all, split_and};
use crate::value::CompOp;

/// `on`, the condition of a semi join of `left` and `right`, over the
/// pair's columns, each conjunct `(a = b) IS NOT FALSE` of a column of
/// each operand that holds no null written `a = b`.
pub(super) fn keyed(on: Cond, (left, right): (&LogicalPlan, &LogicalPlan), catalog: &dyn Catalog) -> Cond {
    let w = left.width();
    let filled = |c: usize| match c.checked_sub(w) {
        Some(r) => never_null(right, r, catalog, 0),
        None => never_null(left, c, catalog, 0),
    };
    let mut conjuncts = Vec::new();
    split_and(on, &mut conjuncts);
    let plain = conjuncts.into_iter().map(|c| match equality_not_false(&c).filter(|&(a, b)| filled(a) && filled(b)) {
        Some((a, b)) => Cond::Compare(Expr::Column(a), CompOp::Eq, Expr::Column(b)),
        None => c,
    });
    and_all(plain.collect()).unwrap_or(Cond::Const(Truth::True))
}

/// The columns `a` and `b` of `c`, if it is `(a = b) IS NOT FALSE`.
fn equality_not_false(c: &Cond) -> Option<(usize, usize)> {
    let Cond::Is(inner, Truth::False, true) = c else { return None };
    let Cond::Compare(Expr::Column(a), CompOp::Eq, Expr::Column(b)) = &**inner else { return None };
    Some((*a, *b))
}

/// Whether column `column` of `plan`'s rows is known to hold no null: a
/// table's column declared so, through the operators that pass a column
/// on as it is and add no row of nulls; `depth` operators down, past
/// `MAX_JUDGED_DEPTH` not known (RULES.md R-CF-6).
pub(super) fn never_null(plan: &LogicalPlan, column: usize, catalog: &dyn Catalog, depth: usize) -> bool {
    if depth > MAX_JUDGED_DEPTH {
        return false;
    }
    let deeper = depth.saturating_add(1);
    match plan {
        LogicalPlan::Scan { table, .. } => catalog.table(table).and_then(|t| t.columns.get(column).map(|c| c.not_null)).unwrap_or(false),
        LogicalPlan::Filter { input, .. }
        | LogicalPlan::Distinct { input }
        | LogicalPlan::Sort { input, .. }
        | LogicalPlan::Limit { input, .. }
        | LogicalPlan::SemiJoin { left: input, .. } => never_null(input, column, catalog, deeper),
        LogicalPlan::Project { exprs, input } => {
            let Some(Expr::Column(c)) = exprs.get(column) else { return false };
            never_null(input, *c, catalog, deeper)
        }
        LogicalPlan::Product { left, right } => match column.checked_sub(left.width()) {
            Some(r) => never_null(right, r, catalog, deeper),
            None => never_null(left, column, catalog, deeper),
        },
        LogicalPlan::Aggregate { .. }
        | LogicalPlan::OuterJoin { .. }
        | LogicalPlan::MarkJoin(_)
        | LogicalPlan::SetOp { .. }
        | LogicalPlan::System { .. }
        | LogicalPlan::Values { .. }
        | LogicalPlan::Recursive { .. }
        | LogicalPlan::WorkTable { .. }
        | LogicalPlan::Transition { .. }
        | LogicalPlan::Unnest(_)
        | LogicalPlan::Window(_)
        | LogicalPlan::Recognize(_)
        | LogicalPlan::Polymorphic(_) => false,
    }
}
