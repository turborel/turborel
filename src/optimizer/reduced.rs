//! \[DESIGN\] The switch of the reduction of joins to semi joins under MIN
//! and MAX (RULES.md D542).
//!
//! The reduction is made of a physical plan, its joins already ordered as
//! they cost least (`physical/blind.rs`), so the rule rewrites no logical
//! plan: it holds the setting OPTIMIZER.SEMI_REDUCTION that turns the
//! reduction on or off, as the other rules' settings do theirs.

use super::{Rule, RuleContext};
use crate::logical::LogicalPlan;

/// The rule's name, as its setting names it.
pub const SEMI_REDUCTION: &str = "semi_reduction";

/// The rule whose setting turns the reduction on or off (see the module's
/// documentation).
pub struct SemiReduction;

impl Rule for SemiReduction {
    fn name(&self) -> &'static str {
        SEMI_REDUCTION
    }

    fn apply(&self, _plan: &LogicalPlan, _ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        None
    }
}
