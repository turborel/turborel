//! \[DESIGN\] Inner joins through independent index-coordinate bounds.
//! Each outer row supplies comparisons to the index; the complete join
//! condition still checks each visible candidate. A raising condition or
//! inner filter cannot take this path (RULES.md D15, D35).

use super::probed::{Choice, planned};
use super::{Read, alike, read_table};
use crate::catalog::IndexDesc;
use crate::logical::{Cond, Expr, LogicalPlan, SortKey, split_and};
use crate::physical::join::Flip;
use crate::physical::shown::Shown;
use crate::physical::{Built, JoinComparison, JoinProbe, PhysicalPlanner, of_left};
use crate::storage::cost_of;
use crate::value::CompOp;

impl PhysicalPlanner<'_> {
    pub(in crate::physical) fn comparison_join(
        &self,
        cond: &Cond,
        (left, right): (&LogicalPlan, &LogicalPlan),
        want: &[SortKey],
    ) -> Option<Built> {
        let flip = Flip { width: left.width(), right_width: right.width() };
        let reversed = flip.cond(cond.clone());
        let normal = self.coordinate_probe((left, right), cond);
        let swapped = self.coordinate_probe((right, left), &reversed);
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

    fn coordinate_probe(&self, (outer, inner): (&LogicalPlan, &LogicalPlan), cond: &Cond) -> Option<Choice> {
        let read = read_table(inner)?;
        let comparisons = crossed(cond, outer.width());
        let outer_rows = self.rows_of(outer);
        let mut best: Option<Choice> = None;
        for ix in self.db.catalog.indexes(read.table) {
            // [EXTENSION] a global index is probed by equalities alone (RULES.md D157).
            if self.db.building.contains(&ix.name) || ix.is_collated() || crate::catalog::is_global_index(&ix.name) {
                continue;
            }
            let Some(bindings) = self.coordinate_columns((outer, &read), ix, &comparisons) else { continue };
            let choice = self.coordinate_cost(&read, ix, bindings, outer_rows)?;
            let rows = cost_of(self.db.storage_of(read.table).ok()?.row_count());
            if choice.total >= outer_rows * rows {
                continue;
            }
            if best.as_ref().is_none_or(|b| choice.total < b.total) {
                best = Some(choice);
            }
        }
        best
    }

    fn coordinate_columns(
        &self,
        (outer, read): (&LogicalPlan, &Read<'_>),
        ix: &IndexDesc,
        comparisons: &[JoinComparison],
    ) -> Option<Vec<JoinComparison>> {
        let desc = self.db.catalog.table(read.table)?;
        let kind = self.db.methods.get(&ix.method)?;
        let table_column = |column: usize| match &read.through {
            Some((columns, _)) => columns.get(column).copied(),
            None => Some(column),
        };
        let mut bindings = Vec::new();
        for c in comparisons {
            let table = table_column(c.column)?;
            let Some(column) = ix.columns.iter().position(|&i| i == table) else { continue };
            let right_type = &desc.columns.get(table)?.data_type;
            if self.column_type(outer, c.outer).is_some_and(|t| alike(&t, right_type)) {
                bindings.push(JoinComparison { column, op: c.op, outer: c.outer });
            }
        }
        let operators: Vec<_> = bindings.iter().map(|c| (c.column, c.op)).collect();
        if bindings.is_empty() || !kind.supports_columns(ix.columns.len(), &operators) {
            return None;
        }
        for (at, &column) in ix.columns.iter().enumerate() {
            if !bindings.iter().any(|c| c.column == at) && !desc.columns.get(column)?.not_null {
                return None;
            }
        }
        Some(bindings)
    }

    fn coordinate_cost(&self, read: &Read<'_>, ix: &IndexDesc, comparisons: Vec<JoinComparison>, outer: f64) -> Option<Choice> {
        let model = self.weights();
        let share = comparisons.iter().map(|c| if c.op == CompOp::Eq { model.equal_selectivity } else { model.range_selectivity });
        let share = share.product::<f64>();
        self.join_cost(read, ix, JoinProbe::Columns(comparisons.into_boxed_slice()), (outer, share))
    }
}

/// Comparisons oriented as inner coordinate OP outer value. Conjuncts
/// of one operand alone remain in the full condition, never probe bounds.
fn crossed(cond: &Cond, width: usize) -> Vec<JoinComparison> {
    let mut conjuncts = Vec::new();
    split_and(cond.clone(), &mut conjuncts);
    conjuncts
        .into_iter()
        .filter_map(|c| {
            let Cond::Compare(Expr::Column(a), op, Expr::Column(b)) = c else { return None };
            if op == CompOp::Ne {
                return None;
            }
            if a < width && b >= width {
                Some(JoinComparison { column: b.saturating_sub(width), op: op.flip(), outer: a })
            } else if b < width && a >= width {
                Some(JoinComparison { column: a.saturating_sub(width), op, outer: b })
            } else {
                None
            }
        })
        .collect()
}
