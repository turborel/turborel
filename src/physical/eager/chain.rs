//! \[DESIGN\] (RULES.md D660) The inner joins under a grouping, taken
//! apart: its relations, each a plan that is no product and no filter
//! over one, at its first column of the chain's; and the conjuncts of the
//! filters over its products, over the chain's columns. Made again of
//! them, a relation replaced, it is the conjuncts' filter over the
//! relations' products, which the planner orders as it orders any chain.

use crate::logical::{Cond, LogicalPlan, and_all, map_cond_columns, split_and};
use crate::optimizer::is_guarded;

/// \[DESIGN\] The deepest chain taken apart (RULES.md R-CF-6): more
/// products than the join enumeration plans.
const MAX_DEPTH: usize = 256;

/// A relation of the chain: its plan, its first column of the chain's,
/// and how many it has.
#[derive(Debug, Clone)]
pub(super) struct Leaf {
    pub(super) plan: LogicalPlan,
    pub(super) start: usize,
    pub(super) width: usize,
}

impl Leaf {
    /// Whether the chain's column `c` is one of it.
    pub(super) fn holds(&self, c: usize) -> bool {
        (self.start..self.start.saturating_add(self.width)).contains(&c)
    }
}

/// The chain: its relations, in the order of their columns, and its
/// conjuncts.
#[derive(Debug, Clone, Default)]
pub(super) struct Chain {
    pub(super) leaves: Vec<Leaf>,
    pub(super) conds: Vec<Cond>,
}

impl Chain {
    /// `plan` taken apart, if it is a chain of two relations or more.
    pub(super) fn of(plan: &LogicalPlan) -> Option<Chain> {
        if !joins(plan, 0) {
            return None;
        }
        let mut chain = Chain::default();
        chain.flatten(plan, 0, 0)?;
        (chain.leaves.len() > 1).then_some(chain)
    }

    /// Adds the relations and conjuncts of `plan`, its columns from
    /// `start`.
    fn flatten(&mut self, plan: &LogicalPlan, start: usize, depth: usize) -> Option<()> {
        if depth > MAX_DEPTH {
            return None;
        }
        let deeper = depth.saturating_add(1);
        if let LogicalPlan::Product { left, right } = plan {
            self.flatten(left, start, deeper)?;
            return self.flatten(right, start.saturating_add(left.width()), deeper);
        }
        if let LogicalPlan::Filter { cond, input } = plan
            && joins(plan, depth)
        {
            split_and(map_cond_columns(cond.clone(), &|i| i.saturating_add(start)), &mut self.conds);
            return self.flatten(input, start, deeper);
        }
        self.leaves.push(Leaf { plan: plan.clone(), start, width: plan.width() });
        Some(())
    }

    /// The conjuncts' filter over the products of `leaves`, in their
    /// order, the conjuncts `conds`.
    pub(super) fn made(leaves: Vec<LogicalPlan>, conds: Vec<Cond>) -> Option<LogicalPlan> {
        let mut leaves = leaves.into_iter();
        let first = leaves.next()?;
        let product = leaves.fold(first, |left, right| LogicalPlan::Product { left: Box::new(left), right: Box::new(right) });
        Some(match and_all(conds) {
            Some(cond) => LogicalPlan::Filter { cond, input: Box::new(product) },
            None => product,
        })
    }
}

/// Whether `plan` joins: a product, or a filter, not a guard
/// (`optimizer::is_guarded`), over one that joins, at nesting `depth`.
fn joins(plan: &LogicalPlan, depth: usize) -> bool {
    if depth > MAX_DEPTH {
        return false;
    }
    if let LogicalPlan::Filter { input, .. } = plan {
        return !is_guarded(plan) && joins(input, depth.saturating_add(1));
    }
    matches!(plan, LogicalPlan::Product { .. })
}
