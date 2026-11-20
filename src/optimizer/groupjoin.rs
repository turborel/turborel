//! \[DESIGN\] (RULES.md D490, D36) The switch of groupjoins. A groupjoin
//! is chosen as the physical plan is made, of the operators the join and
//! the grouping were planned as (`physical/groupjoin.rs`), so it rewrites
//! nothing of the logical plan; this rule holds its setting,
//! OPTIMIZER.GROUPJOIN, as every rule holds its own, so a session turns
//! groupjoins off as it turns off any rewrite.

use super::{Rule, RuleContext};
use crate::logical::LogicalPlan;
pub use crate::physical::GROUPJOIN;

/// \[DESIGN\] (RULES.md D490) The rule whose setting turns groupjoins on
/// or off; it leaves every logical plan as it is.
pub struct GroupJoins;

impl Rule for GroupJoins {
    fn name(&self) -> &'static str {
        GROUPJOIN
    }

    fn apply(&self, _plan: &LogicalPlan, _ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        None
    }
}
