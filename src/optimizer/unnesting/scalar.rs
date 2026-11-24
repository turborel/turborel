//! \[DESIGN\] A comparison with a subquery of one row at most made a semi
//! join (RULES.md D773). A conjunct of WHERE or HAVING that compares a
//! value of the node's row with a column of an uncorrelated scalar or
//! row subquery, as `NO_O_ID = (SELECT MIN(NO_O_ID) FROM ...)` does, is
//! true of a row exactly when the subquery has a row and the comparison
//! with it is true [X3.135-1986 §5.11 GR1, §5.24 GR2]: of no row the
//! value is null and the comparison unknown, which WHERE keeps no row of.
//! So the node's rows are those of the semi join of its input with the
//! subquery's rows on the comparisons, when the subquery gives one row at
//! most by its shape: an ungrouped set function's, or one fetched first.
//! Of several such subqueries the right is the product of their rows, one
//! at most, so a bound of each, as STOCK-LEVEL's `OL_O_ID < (...)` and
//! `OL_O_ID >= (...)`, bounds one probe.
//! A subquery that could give more would raise its cardinality error
//! [ISO/IEC 9075:1992 §7.11 GR1], which no join raises, so it is left.
//!
//! The planner may then probe the input's index from the subquery's row
//! (`physical/semi/probed.rs`), its value read once, as a range of keys
//! when the comparison is one of order. As the other semi joins are, it is
//! made only of a subquery that cannot raise, is pure (D318), and of
//! comparisons that cannot raise, and the subquery may stand nowhere else
//! in the condition.

use crate::logical::{Cond, Expr, LogicalPlan, Subquery, and_all, split_and};
use crate::optimizer::RuleContext;
use crate::value::CompOp;

use super::pure;

/// The filter of `cond` over `input` made the semi join of its input with
/// the product of the subqueries of one row at most that its conjuncts
/// compare its rows' values with, those it may be made of, if any.
pub(super) fn joined(cond: &Cond, input: &LogicalPlan, ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
    let mut conjuncts = Vec::new();
    split_and(cond.clone(), &mut conjuncts);
    let mut subs: Vec<&Subquery> = Vec::new();
    for sub in conjuncts.iter().filter_map(compared_subquery) {
        if !subs.iter().any(|s| s.id() == sub.id()) && joinable(&conjuncts, sub, ctx) {
            subs.push(sub);
        }
    }
    let width = input.width();
    let (mut on, mut rest, mut right) = (Vec::new(), Vec::new(), None::<LogicalPlan>);
    let mut starts = Vec::new();
    for sub in &subs {
        starts.push(right.as_ref().map_or(width, |r| width.saturating_add(r.width())));
        right = Some(match right {
            Some(r) => LogicalPlan::Product { left: Box::new(r), right: Box::new(sub.plan.clone()) },
            None => sub.plan.clone(),
        });
    }
    for c in &conjuncts {
        let compared = subs.iter().zip(&starts).find_map(|(sub, start)| comparison_with(c, sub.id()).map(|w| (w, *start)));
        match compared {
            Some(((x, op, column), start)) => on.push(Cond::Compare(x, op, Expr::Column(start.checked_add(column)?))),
            None => rest.push(c.clone()),
        }
    }
    let left = match and_all(rest) {
        Some(c) => LogicalPlan::Filter { cond: c, input: Box::new(input.clone()) },
        None => input.clone(),
    };
    Some(LogicalPlan::SemiJoin { anti: false, cond: and_all(on)?, left: Box::new(left), right: Box::new(right?) })
}

/// Whether `sub` may be joined so: uncorrelated, of one row at most, pure
/// and raising nothing, each of its uses among `conjuncts` such a
/// comparison.
fn joinable(conjuncts: &[Cond], sub: &Subquery, ctx: &RuleContext<'_>) -> bool {
    let raises = pure::plan_raises(&sub.plan, ctx);
    if !sub.outer.is_empty() || !sub.plan.one_row_at_most() || raises || !pure::plan_pure(&sub.plan) {
        return false;
    }
    conjuncts.iter().all(|c| comparison_with(c, sub.id()).is_some() || !uses(c, sub.id()))
}

/// The subquery `c` compares a value with, if it is such a comparison.
fn compared_subquery(c: &Cond) -> Option<&Subquery> {
    let Cond::Compare(a, _, b) = c else { return None };
    match (a, b) {
        (Expr::Subquery(s, _), _) | (_, Expr::Subquery(s, _)) => Some(s),
        _ => None,
    }
}

/// Of `c`, a comparison of a value with column `column` of the subquery
/// `id`, the value, which holds no subquery and cannot raise, the
/// operator, written with the value first, and the column.
fn comparison_with(cond: &Cond, id: u64) -> Option<(Expr, CompOp, usize)> {
    let Cond::Compare(first, op, second) = cond else { return None };
    let (value, op, sub, column) = match (first, second) {
        (value, Expr::Subquery(sub, column)) => (value, *op, sub, *column),
        (Expr::Subquery(sub, column), value) => (value, op.flip(), sub, *column),
        _ => return None,
    };
    let plain = !value.can_raise() && value.all_subqueries().is_empty();
    (sub.id() == id && plain).then(|| (value.clone(), op, column))
}

/// Whether `c` uses the subquery `id` anywhere.
fn uses(c: &Cond, id: u64) -> bool {
    c.all_subqueries().iter().any(|s| s.id() == id)
}
