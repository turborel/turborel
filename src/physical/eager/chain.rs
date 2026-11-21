//! \[DESIGN\] (RULES.md D660) The inner joins under a grouping, taken
//! apart: its relations, each a plan that is no product and no filter
//! over one, at its first column of the chain's; and the conjuncts of the
//! filters over its products, over the chain's columns. Made again of
//! them, a relation replaced, it is the relations' products, each
//! conjunct over the lowest that reads it, which the planner orders as it
//! orders any chain.

use crate::logical::{Cond, LogicalPlan, and_all, cond_columns, map_cond_columns, split_and};
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

    /// The products of `leaves`, in their order, each conjunct of `conds`
    /// in a filter over the lowest product, or the first relation, that
    /// gives every column it reads, as predicate pushdown leaves a chain:
    /// a chain planned as written then joins each relation by its own
    /// conjuncts rather than pairing it with each row of the last.
    pub(super) fn made(leaves: Vec<LogicalPlan>, conds: Vec<Cond>) -> Option<LogicalPlan> {
        let mut leaves = leaves.into_iter();
        let mut plan = leaves.next()?;
        let mut pending: Vec<(usize, Cond)> = conds.into_iter().map(|c| (last_column(&c), c)).collect();
        plan = placed(plan, &mut pending);
        for right in leaves {
            plan = placed(LogicalPlan::Product { left: Box::new(plan), right: Box::new(right) }, &mut pending);
        }
        let rest = and_all(pending.into_iter().map(|(_, c)| c).collect());
        Some(match rest {
            Some(cond) => LogicalPlan::Filter { cond, input: Box::new(plan) },
            None => plan,
        })
    }
}

/// The last column `c` reads, or 0 if it reads none.
fn last_column(c: &Cond) -> usize {
    let mut reads = Vec::new();
    cond_columns(c, &mut reads);
    reads.into_iter().max().unwrap_or(0)
}

/// `plan` under the conjuncts of `pending` that read only its columns,
/// taken out of `pending`.
fn placed(plan: LogicalPlan, pending: &mut Vec<(usize, Cond)>) -> LogicalPlan {
    let width = plan.width();
    let (here, later): (Vec<_>, Vec<_>) = std::mem::take(pending).into_iter().partition(|(last, _)| *last < width);
    *pending = later;
    match and_all(here.into_iter().map(|(_, c)| c).collect()) {
        Some(cond) => LogicalPlan::Filter { cond, input: Box::new(plan) },
        None => plan,
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
