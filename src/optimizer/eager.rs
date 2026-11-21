//! \[DESIGN\] (RULES.md D660, D36) The switch of eager aggregation. A
//! grouping over joins is planned with a table grouped first as the
//! physical plan is made, by the costs of both plans
//! (`physical/eager.rs`), so this rule rewrites nothing of the logical
//! plan; it holds the setting OPTIMIZER.EAGER_AGGREGATION, as every rule
//! holds its own, so a session turns eager aggregation off as it turns
//! off any rewrite.

use super::{Rule, RuleContext};
use crate::logical::LogicalPlan;
pub use crate::physical::EAGER_AGGREGATION;

/// \[DESIGN\] (RULES.md D660) The rule whose setting turns eager
/// aggregation on or off; it leaves every logical plan as it is.
pub struct EagerAggregation;

impl Rule for EagerAggregation {
    fn name(&self) -> &'static str {
        EAGER_AGGREGATION
    }

    fn apply(&self, _plan: &LogicalPlan, _ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        None
    }
}
