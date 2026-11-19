//! Geometry outer joins through envelope candidates [ISO/IEC 9075:1992
//! §7.5] (D135). A LEFT join's rows each probe the right operand's table,
//! a RIGHT join's right rows the left's, under a projection putting the
//! columns back in order; a FULL join probes either way, and then reads
//! the probed table for the rows no probe matched (`exec/join/indexed`).
//! A null or empty outer geometry probes nothing and keeps its row with
//! nulls, as the condition is not true of any pair of it. Every candidate
//! pair is judged by the whole condition, which no conjunct beside the
//! spatial one may make raise, so pruning drops only pairs it is not true
//! of and errors are raised as the product's would be (D15).

use super::super::probed::{Preserve, planned};
use crate::logical::{Cond, LogicalPlan, OuterJoin, SortKey};
use crate::physical::join::Flip;
use crate::physical::shown::Shown;
use crate::physical::{Built, PhysicalPlanner, of_left};

impl PhysicalPlanner<'_> {
    /// The outer join `kind` of `left` and `right` on `cond`, through an
    /// R-tree of one operand's table, if `cond` has a windowed spatial
    /// conjunct an index serves and probing is estimated cheaper.
    pub(in crate::physical) fn spatial_outer_join(
        &self,
        (kind, cond): (OuterJoin, &Cond),
        (left, right): (&LogicalPlan, &LogicalPlan),
        want: &[SortKey],
    ) -> Option<Built> {
        let flip = Flip { width: left.width(), right_width: right.width() };
        let names = self.shown.then(|| left.names().into_iter().chain(right.names()).collect::<Vec<_>>());
        let shown = Shown::of(names.as_deref());
        let (preserve, normal, swapped) = match kind {
            OuterJoin::Left => (Preserve::Outer, true, false),
            OuterJoin::Right => (Preserve::Outer, false, true),
            OuterJoin::Full => (Preserve::Both, true, true),
        };
        let reversed = flip.cond(cond.clone());
        let normal = normal.then(|| self.geometry_probe((left, right), cond, preserve)).flatten();
        let swapped = swapped.then(|| self.geometry_probe((right, left), &reversed, preserve)).flatten();
        if swapped.as_ref().is_some_and(|s| normal.as_ref().is_none_or(|n| s.total < n.total)) {
            return Some(Built::unordered(self.swapped(swapped?, right, (reversed, flip, shown))));
        }
        let mut choice = normal?;
        // A traversal pairs rows in the trees' order, not the left's.
        choice.paired = choice.paired.filter(|_| want.is_empty());
        // The unmatched rows of a FULL join's table follow the rest.
        let wanted = if kind == OuterJoin::Full { &[] } else { of_left(want, left.width()) };
        let outer = self.build_node(left, wanted);
        let ordered = !wanted.is_empty() && outer.ordered;
        Some(Built { plan: planned(choice, outer.plan, (cond.clone(), left.width()), shown), ordered })
    }
}
