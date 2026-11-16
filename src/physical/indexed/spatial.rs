//! Inner geometry joins through envelope candidates. Both source columns
//! enforce the same SRID, other conjuncts and the inner filter cannot
//! raise, and the full ON condition checks every visible pair (D15, D82).

use super::probed::{Choice, planned};
use super::{Read, read_table};
use crate::catalog::IndexDesc;
use crate::functions::ScalarRef;
use crate::geometry::fixed_srid;
use crate::index::Predicate;
use crate::logical::{Cond, Expr, LogicalPlan, SortKey, split_and};
use crate::physical::join::Flip;
use crate::physical::shown::Shown;
use crate::physical::spatial::intersection;
use crate::physical::{Built, JoinProbe, PhysicalPlanner, of_left};
use crate::storage::cost_of;

struct Binding {
    function: ScalarRef,
    outer: usize,
    inner: usize,
    argument: usize,
}

impl PhysicalPlanner<'_> {
    pub(in crate::physical) fn spatial_join(
        &self,
        cond: &Cond,
        (left, right): (&LogicalPlan, &LogicalPlan),
        want: &[SortKey],
    ) -> Option<Built> {
        let flip = Flip { width: left.width(), right_width: right.width() };
        let reversed = flip.cond(cond.clone());
        let normal = self.geometry_probe((left, right), cond);
        let swapped = self.geometry_probe((right, left), &reversed);
        let swap = swapped.as_ref().is_some_and(|s| normal.as_ref().is_none_or(|n| s.total < n.total));
        let names = self.shown.then(|| left.names().into_iter().chain(right.names()).collect::<Vec<_>>());
        let shown = Shown::of(names.as_deref());
        if swap {
            let choice = swapped?;
            let outer = self.build_node(right, &[]).plan;
            let flipped_names = flip.names(shown);
            let flipped = Shown::of(flipped_names.as_deref());
            let plan = planned(choice, outer, (reversed, right.width()), flipped);
            return Some(Built::unordered(flip.restored(plan, flipped)));
        }
        let choice = normal?;
        let wanted = of_left(want, left.width());
        let outer = self.build_node(left, wanted);
        let ordered = !wanted.is_empty() && outer.ordered;
        let plan = planned(choice, outer.plan, (cond.clone(), left.width()), shown);
        Some(Built { plan, ordered })
    }

    fn geometry_probe(&self, (outer, inner): (&LogicalPlan, &LogicalPlan), cond: &Cond) -> Option<Choice> {
        let binding = crossed(cond, outer.width())?;
        let read = read_table(inner)?;
        let srid = fixed_srid(&self.column_type(outer, binding.outer)?)?;
        let inner_srid = fixed_srid(&self.column_type(inner, binding.inner)?)?;
        if srid != inner_srid {
            return None;
        }
        let outer_rows = self.rows_of(outer)?;
        let rows = cost_of(self.db.storage_of(read.table).ok()?.row_count());
        let mut best: Option<Choice> = None;
        for ix in self.db.catalog.indexes(read.table) {
            let Some(predicate) = self.geometry_key(&read, ix, &binding) else { continue };
            let probe = JoinProbe::Predicate { predicate, outer: binding.outer };
            // Until geometry statistics exist, use the two-dimensional
            // range estimate. It affects cost only, never the residual.
            let range = self.db.cost().range_selectivity;
            let choice = self.join_cost(&read, ix, probe, (outer_rows, range * range))?;
            if choice.total >= outer_rows * rows {
                continue;
            }
            if best.as_ref().is_none_or(|b| choice.total < b.total) {
                best = Some(choice);
            }
        }
        best
    }

    fn geometry_key(&self, read: &Read<'_>, ix: &IndexDesc, binding: &Binding) -> Option<Box<Predicate>> {
        if self.db.building.contains(&ix.name) || ix.is_collated() {
            return None;
        }
        let column = match &read.through {
            Some((columns, _)) => *columns.get(binding.inner)?,
            None => binding.inner,
        };
        if ix.columns.as_slice() != [column] {
            return None;
        }
        self.db.index_of(&ix.name).ok()?;
        let kind = self.db.methods.get(&ix.method)?;
        let types = ix.key_types(self.db.catalog.table(read.table)?)?;
        let predicate = Predicate { function: binding.function.clone(), column: 0, argument: binding.argument };
        kind.supports_predicate(&predicate, &types).then(|| Box::new(predicate))
    }
}

fn crossed(cond: &Cond, width: usize) -> Option<Binding> {
    let mut conjuncts = Vec::new();
    split_and(cond.clone(), &mut conjuncts);
    let mut found = None;
    for c in &conjuncts {
        if let Some((function, args)) = intersection(c) {
            if found.is_some() {
                return None;
            }
            let [Expr::Column(a), Expr::Column(b)] = args else { return None };
            let (outer, inner, argument) = if *a < width && *b >= width {
                (*a, b.saturating_sub(width), 1)
            } else if *b < width && *a >= width {
                (*b, a.saturating_sub(width), 0)
            } else {
                return None;
            };
            found = Some(Binding { function: function.clone(), outer, inner, argument });
        } else if c.can_raise() {
            return None;
        }
    }
    found
}
