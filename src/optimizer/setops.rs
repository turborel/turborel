//! \[DESIGN\] A filter moved into both operands of a set operation (RULES.md
//! D374). The rows of UNION ALL are each operand's rows, every one of them
//! [ISO/IEC 9075-2:2023 §7.17 GR3b iii], so a filter over it keeps of each
//! operand the rows it keeps, and evaluates its condition on the same rows
//! once each, whatever the condition is. UNION keeps one row of each set
//! of rows not distinct, INTERSECT and EXCEPT a row of the first operand by
//! how many rows not distinct from it each operand has, and with ALL as
//! many copies as those counts give [§7.17 GR3b]. A condition that treats
//! rows not distinct alike (`views.rs` `alike`) keeps all of such a set or
//! none, in each operand as over the result, so it keeps the same rows
//! when it filters each operand first. Over UNION it is then evaluated on
//! rows each equal to a row of the result, and raises of them as of that
//! row; over INTERSECT and EXCEPT also on rows of the result's operand
//! that the result would not have, so there it must not raise (RULES.md
//! D15). A set operation that compares rows by sort keys is left: its
//! values are not its sort keys, and two rows alike by the keys can differ
//! in a condition of the values.

use super::views::alike;
use super::{Rule, RuleContext};
use crate::logical::{LogicalPlan, SetOp};

/// The rule's name, as EXPLAIN shows it and its setting names it.
pub const PUSH_FILTER_THROUGH_SET_OPERATION: &str = "push_filter_through_set_operation";

/// A filter over a set operation moved into both its operands, as the
/// module says.
pub struct PushFilterThroughSetOperation;

impl Rule for PushFilterThroughSetOperation {
    fn name(&self) -> &'static str {
        PUSH_FILTER_THROUGH_SET_OPERATION
    }
    fn apply(&self, plan: &LogicalPlan, _ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        let LogicalPlan::Filter { cond, input } = plan else { return None };
        let LogicalPlan::SetOp { op, all, compared, left, right } = &**input else { return None };
        if compared.is_some() {
            return None;
        }
        let moves = match op {
            SetOp::Union => *all || alike(cond),
            SetOp::Intersect | SetOp::Except => alike(cond) && !cond.can_raise(),
        };
        if !moves {
            return None;
        }
        let filtered = |operand: &LogicalPlan| Box::new(LogicalPlan::Filter { cond: cond.clone(), input: Box::new(operand.clone()) });
        Some(LogicalPlan::SetOp { op: *op, all: *all, compared: None, left: filtered(left), right: filtered(right) })
    }
}
