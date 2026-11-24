//! \[DESIGN\] An outer join that is an operand of the product a WHERE
//! filters (RULES.md D1061). A rule that moves an outer join onto the one
//! operand of a product its condition reads leaves the WHERE clause over
//! the product, no longer over the join. A conjunct of it that is never
//! true while the columns the join gives nulls are null
//! (`rejecting.rs`) still keeps none of the rows the join gives nulls, as
//! each row of the product holds one row of the join [X3.135-1986 §5.20
//! GR2b]. So the join is made the join it is where it stands, the
//! product's columns as they were.

use super::rejecting::never_true;
use crate::logical::{Cond, LogicalPlan, MAX_JUDGED_DEPTH};

/// `plan`, an operand of a product filtered by `conjuncts` whose columns
/// begin at `offset` of the product's, an outer join among its operands
/// made the join the conjuncts leave of it, the first found; none if
/// there is none. Past `MAX_JUDGED_DEPTH` products deep none is looked
/// for (RULES.md R-CF-6).
pub(super) fn made(conjuncts: &[Cond], plan: &LogicalPlan, (offset, depth): (usize, usize)) -> Option<LogicalPlan> {
    if depth > MAX_JUDGED_DEPTH {
        return None;
    }
    let deeper = depth.saturating_add(1);
    match plan {
        LogicalPlan::Product { left, right } => {
            if let Some(made) = made(conjuncts, left, (offset, deeper)) {
                return Some(LogicalPlan::Product { left: Box::new(made), right: right.clone() });
            }
            let made = made(conjuncts, right, (offset.saturating_add(left.width()), deeper))?;
            Some(LogicalPlan::Product { left: left.clone(), right: Box::new(made) })
        }
        LogicalPlan::OuterJoin { kind, cond, left, right } => {
            let middle = offset.saturating_add(left.width());
            let end = middle.saturating_add(right.width());
            let rejected = |from: usize, to: usize| conjuncts.iter().any(|c| never_true(c, &|column| column >= from && column < to));
            let no_left_nulls = kind.keeps_right() && rejected(offset, middle);
            let no_right_nulls = kind.keeps_left() && rejected(middle, end);
            let kind = super::left_of(*kind, no_left_nulls, no_right_nulls)?;
            Some(super::joined(kind, cond.clone(), (left.clone(), right.clone())))
        }
        LogicalPlan::Scan { .. }
        | LogicalPlan::Filter { .. }
        | LogicalPlan::SemiJoin { .. }
        | LogicalPlan::MarkJoin(_)
        | LogicalPlan::Aggregate { .. }
        | LogicalPlan::Project { .. }
        | LogicalPlan::Distinct { .. }
        | LogicalPlan::SetOp { .. }
        | LogicalPlan::Values { .. }
        | LogicalPlan::System { .. }
        | LogicalPlan::Sort { .. }
        | LogicalPlan::Limit { .. }
        | LogicalPlan::Recursive { .. }
        | LogicalPlan::Unnest(..)
        | LogicalPlan::Window(..)
        | LogicalPlan::Recognize(..)
        | LogicalPlan::Polymorphic(..)
        | LogicalPlan::WorkTable { .. }
        | LogicalPlan::Transition { .. } => None,
    }
}
