//! \[DESIGN\] An outer join as a range join (RULES.md D580): a LEFT join
//! whose left rows search its right operand sorted, each left row no pair
//! is kept of given with nulls [ISO/IEC 9075:1992 §7.5 GR1c, GR3-GR5]; a RIGHT join
//! as the LEFT one of its operands swapped, put back in order by a
//! projection. A FULL join keeps the right's unmatched rows too, which no
//! search finds, and stays as it is (`physical/join.rs`). The whole `<join
//! condition>` must be unable to raise, so no conjunct is an operand's own
//! (`Plan::OuterJoin`): each is judged only of the pairs it is of.

use super::planned::{Made, range_plan};
use crate::logical::{Cond, LogicalPlan, OuterJoin, SortKey};
use crate::physical::join::Flip;
use crate::physical::shown::Shown;
use crate::physical::{Built, PhysicalPlanner, of_left};

impl PhysicalPlanner<'_> {
    /// The outer join `kind` of `left` and `right` on `cond` as a range
    /// join, if it is LEFT or RIGHT and may be one, estimated cheapest.
    pub(in crate::physical) fn range_outer_join(
        &self,
        (kind, cond): (OuterJoin, &Cond),
        (left, right): (&LogicalPlan, &LogicalPlan),
        want: &[SortKey],
    ) -> Option<Built> {
        // The preserved operand searches: the left of a LEFT join, the
        // right of a RIGHT one, as the left of the join swapped.
        let (probe, build) = match kind {
            OuterJoin::Left => (left, right),
            OuterJoin::Right => (right, left),
            OuterJoin::Full => return None,
        };
        let flip = Flip { width: left.width(), right_width: right.width() };
        let swapped = kind == OuterJoin::Right;
        let on = if swapped { flip.cond(cond.clone()) } else { cond.clone() };
        let est = self.estimating(|e| e.outer_card((kind, cond), (left, right))).rows;
        let want_probe = if swapped { &[] } else { of_left(want, left.width()) };
        let way = self.range_way(&on, (probe, build), (est, true))?;
        if way.cost >= self.joined_otherwise(&on, (probe, build), &way.ranged) {
            return None;
        }
        let names = self.shown.then(|| left.names().into_iter().chain(right.names()).collect::<Vec<String>>());
        let shown = Shown::of(names.as_deref());
        let flipped = flip.names(shown);
        let of_pair = if swapped { Shown::of(flipped.as_deref()) } else { shown };
        let (p, b) = (self.build_node(probe, want_probe), self.build_node(build, &[]));
        let made = Made { outer: true, widths: (probe.width(), build.width()), full: on, shown: of_pair, est };
        let plan = range_plan(way.ranged, (p.plan, b.plan), made);
        if swapped {
            return Some(Built::unordered(flip.restored(plan, of_pair)));
        }
        Some(Built { plan, ordered: !want_probe.is_empty() && p.ordered })
    }
}
