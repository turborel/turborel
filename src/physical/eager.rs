//! \[DESIGN\] (RULES.md D660) Eager aggregation: a grouping over inner
//! joins computed in part below them, one relation of its chain grouped
//! first by the columns the joins and the grouping read of it, as Yan and
//! Larson, "Eager Aggregation and Lazy Aggregation" (VLDB 1995), push a
//! group-by past a join, so the joins pair the early groups, fewer rows,
//! where they would pair every row of it. Each set function is computed
//! in parts below and merged above, or above of the values the early
//! groups pass up, multiplied by the rows each stands for where
//! duplicates count (`eager/classify.rs`), so the results are the
//! grouping's.
//!
//! Each relation is tried in turn whose grouping first is estimated to
//! leave at most half its rows (`MIN_REDUCTION`), Yan and Larson's rule of
//! abandoning a grouping by a key widened: the grouping's plan made again
//! with it grouped first (`eager/rewrite.rs`), its joins ordered by the
//! planner as any chain's, and weighed against the plan as written
//! (`eager/cost.rs`). The least work is taken if it is less than the
//! plan as written's by the gain PLANNER.EAGER_GAIN sets (RULES.md D663),
//! as Fent and Neumann (2021) find eager aggregation wins or loses by
//! costs. The setting OPTIMIZER.EAGER_AGGREGATION turns it off.
//!
//! Left alone: a grouping of empty grouping sets, whose groups of no rows
//! the early grouping would not make; one grouping by a collation; and
//! a chain under a guard or an outer join, whose operands it treats as
//! relations, whole.

mod chain;
mod classify;
mod cost;
mod rewrite;

use chain::Chain;
use classify::Cx;
use rewrite::{Grouping, rewritten};

use super::cardinality::Estimates;
use super::cost::runtime;
use super::{Built, PhysicalPlanner, Plan};
use crate::functions::{AggregateRef, ColumnarKind};
use crate::lexer::kw;
use crate::logical::{LogicalPlan, SortKey, plan_column_type};
use crate::settings::EAGER_GAIN;

/// The name of the rule whose setting turns eager aggregation on or off,
/// OPTIMIZER.EAGER_AGGREGATION (`optimizer/eager.rs`).
pub const EAGER_AGGREGATION: &str = "eager_aggregation";

/// \[DESIGN\] The default of PLANNER.EAGER_GAIN: a plan grouped first is
/// taken if estimated to do at most this much of the work of the plan as
/// written. It lies between the estimated ratios of the bench's queries
/// that ran faster grouped first and those that did not
/// (`bench/analytics/eager.rs`), short of 1 as a margin for the
/// estimates of groups, which are less sure than those of rows.
pub const DEFAULT_EAGER_GAIN: f64 = 0.8;

/// \[DESIGN\] The fewest rows of a relation each of its early groups must
/// be estimated to take for it to be grouped first: fewer, a grouping by
/// what is nearly a key, which leaves the join as many rows.
const MIN_REDUCTION: f64 = 2.0;

impl PhysicalPlanner<'_> {
    /// `plain`, the plan of `logical`, a grouping, as written; or the plan
    /// of it with a relation of its joins grouped first, if one is
    /// estimated cheaper by the gain (as the module says).
    pub(super) fn eager(&self, logical: &LogicalPlan, want: &[SortKey], plain: Built) -> Built {
        let LogicalPlan::Aggregate { groups, collations, aggs, seeds, input } = logical else { return plain };
        let grouped = !seeds.is_empty() || collations.iter().any(Option::is_some);
        if grouped || !self.db.optimizer.enabled(EAGER_AGGREGATION, &self.db.settings) {
            return plain;
        }
        let (Some(chain), Some(count)) = (Chain::of(input), self.count()) else { return plain };
        let catalog = &*self.db.catalog;
        let types = |c: usize| plan_column_type(input, c, catalog);
        let grouping = Grouping { groups, collations, aggs };
        let filters = runtime::wanted(self.db);
        let (gain, written) = (self.db.settings.number(EAGER_GAIN), cost::work(&plain.plan, filters));
        let mut best: Option<(f64, Built)> = None;
        for x in 0..chain.leaves.len() {
            let cx = Cx { chain: &chain, x, groups, aggs, types: &types, catalog };
            let Some(alt) = cx.shape().and_then(|shape| rewritten(&grouping, (&chain, x), &shape, &count)) else { continue };
            if !self.reduces(&alt, x) {
                continue;
            }
            let built = self.eager_planned(&alt, want);
            let work = cost::work(&built.plan, filters);
            if work < gain * written && best.as_ref().is_none_or(|(least, _)| work < *least) {
                best = Some((work, built));
            }
        }
        best.map_or(plain, |(_, built)| built)
    }

    /// The built-in COUNT, of which each early group's rows are counted.
    fn count(&self) -> Option<AggregateRef> {
        let count = self.db.functions.aggregate(kw::COUNT)?;
        (count.0.columnar().map(|c| c.0) == Some(ColumnarKind::Count)).then_some(count)
    }

    /// Whether the early grouping of relation `x` of `alt`'s chain is
    /// estimated to take `MIN_REDUCTION` rows a group.
    fn reduces(&self, alt: &LogicalPlan, x: usize) -> bool {
        let LogicalPlan::Aggregate { input, .. } = alt else { return false };
        let Some(early) = Chain::of(input).and_then(|c| c.leaves.into_iter().nth(x)) else { return false };
        let LogicalPlan::Aggregate { input: rows, .. } = &early.plan else { return false };
        self.card(rows).rows >= MIN_REDUCTION * self.card(&early.plan).rows
    }

    /// `alt` planned whole, estimated by its own estimator, and shown as a
    /// grouping of a relation grouped first.
    fn eager_planned(&self, alt: &LogicalPlan, want: &[SortKey]) -> Built {
        let estimates = Estimates::over(self.db, alt);
        let mut built = PhysicalPlanner { est: Some(&estimates), ..*self }.build_node(alt, want);
        if let Plan::Aggregate { detail, .. } = &mut built.plan
            && self.shown
        {
            detail.push_str(" (eager: a table grouped first below the joins)");
        }
        built
    }
}

// Unit tests: plans grouped first and left alone (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
