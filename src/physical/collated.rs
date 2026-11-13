//! \[EXTENSION\] A join on the sort keys of a collation (RULES.md D60). A
//! hash join's keys are columns, so a condition equating the sort keys of a
//! column of each operand has them made columns of its operands, which a
//! projection takes away after, and the join is planned as any.

use crate::logical::{Cond, Expr, LogicalPlan, and_all, map_cond_columns, map_expr_columns, split_and};
use crate::value::CompOp;

/// A join whose sort keys are made columns of its operands.
pub(super) struct Lifted {
    /// Its condition, over the operands below.
    pub(super) cond: Cond,
    /// Its left operand, its sort keys after its columns.
    pub(super) left: LogicalPlan,
    /// Its right operand, likewise.
    pub(super) right: LogicalPlan,
    /// What gives the join's own columns of its pairs.
    pub(super) kept: Vec<Expr>,
}

/// The sort keys `c` equates, of a column of each operand, the left
/// `width` columns wide, the left's first, and whether a null matches a
/// null of them, as `c` is IS NOT DISTINCT FROM [ISO/IEC 9075-2:2023
/// §8.15]. A sort key of a null is null, so the keys match as the strings.
fn keys_of(c: &Cond, width: usize) -> Option<(Expr, Expr, bool)> {
    let (first, second, nulls) = match c {
        Cond::Compare(first, CompOp::Eq, second) => (first, second, false),
        Cond::Not(inner) => {
            let Cond::Distinct(first, second) = &**inner else { return None };
            (first, second, true)
        }
        Cond::Compare(..)
        | Cond::Const(_)
        | Cond::Or(..)
        | Cond::And(..)
        | Cond::Is(..)
        | Cond::Distinct(..)
        | Cond::IsNull(..)
        | Cond::Like(..)
        | Cond::Exists(_)
        | Cond::Unique(..)
        | Cond::Quantified(..)
        | Cond::Match { .. }
        | Cond::In(..)
        | Cond::Value(_) => return None,
    };
    let (Expr::SortKey(of_first, _), Expr::SortKey(of_second, _)) = (first, second) else { return None };
    let (Expr::Column(one), Expr::Column(other)) = (&**of_first, &**of_second) else { return None };
    if *one < width && *other >= width {
        Some((first.clone(), second.clone(), nulls))
    } else if *other < width && *one >= width {
        Some((second.clone(), first.clone(), nulls))
    } else {
        None
    }
}

/// The condition matching left column `l` with right column `r`: equal,
/// or if `nulls`, not distinct.
fn matching(l: usize, r: usize, nulls: bool) -> Cond {
    if nulls {
        Cond::Not(Box::new(Cond::Distinct(Expr::Column(l), Expr::Column(r))))
    } else {
        Cond::Compare(Expr::Column(l), CompOp::Eq, Expr::Column(r))
    }
}

/// `e`, the sort key of a column of the right operand, of its own columns.
fn of_right(e: Expr, width: usize) -> Expr {
    map_expr_columns(e, &|c| c.saturating_sub(width))
}

/// `plan` with the columns `keys` after its own.
fn with_keys(plan: &LogicalPlan, keys: Vec<Expr>) -> LogicalPlan {
    let exprs = (0..plan.width()).map(Expr::Column).chain(keys).collect();
    LogicalPlan::Project { exprs, input: Box::new(plan.clone()) }
}

/// The join of `left` and `right` on `cond` as `Lifted`, if `cond` equates
/// sort keys of a column of each.
pub(super) fn lifted(cond: &Cond, left: &LogicalPlan, right: &LogicalPlan) -> Option<Lifted> {
    let (width, right_width) = (left.width(), right.width());
    let mut conjuncts = Vec::new();
    split_and(cond.clone(), &mut conjuncts);
    let (mut lefts, mut rights, mut nulls, mut rest) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for c in conjuncts {
        match keys_of(&c, width) {
            Some((l, r, matches_null)) => {
                lefts.push(l);
                rights.push(of_right(r, width));
                nulls.push(matches_null);
            }
            None => rest.push(c),
        }
    }
    if lefts.is_empty() {
        return None;
    }
    let count = lefts.len();
    let wider = width.saturating_add(count);
    let moved = move |c: usize| if c < width { c } else { c.saturating_add(count) };
    let equal = nulls
        .into_iter()
        .enumerate()
        .map(|(k, matches_null)| matching(width.saturating_add(k), wider.saturating_add(right_width).saturating_add(k), matches_null));
    let conds: Vec<Cond> = equal.chain(rest.into_iter().map(|c| map_cond_columns(c, &moved))).collect();
    let kept = (0..width).chain(wider..wider.saturating_add(right_width)).map(Expr::Column).collect();
    Some(Lifted { cond: and_all(conds)?, left: with_keys(left, lefts), right: with_keys(right, rights), kept })
}
