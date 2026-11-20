//! \[DESIGN\] A geometry join of two tables each with an R-tree on its
//! join column may descend both trees together instead of probing one
//! for each outer row (RULES.md D138). It needs the relationship to be
//! true only of meeting operands, so that pairs of entries whose
//! envelopes meet are every pair it can hold of, and two columns of one
//! SRID, so that pruning hides no error (D85). The outer operand must
//! read its table whole or through a filter that cannot raise, as the
//! traversal reads its rows only through their entries, perhaps under a
//! projection of its columns, as a derived table's is. The join keeps
//! its probes when the cost of the probes and the outer scan is lower,
//! and only if both tables' statistics put the pairs at no more than
//! `PAIRED_MOST` an outer row (D138).

use super::{Binding, Choice, Preserve, Read, read_table};
use crate::catalog::IndexDesc;
use crate::geometry::SpatialOperation;
use crate::logical::LogicalPlan;
use crate::physical::{Paired, PhysicalPlanner};
use crate::storage::cost_of;

/// \[DESIGN\] The most pairs an outer row may be estimated to make, by both
/// columns' envelope grids, for a traversal to be taken. A pair costs the
/// traversal 3.7 to 9.8 µs, by which table it pairs from, against about
/// 5 µs for a probe's descent, so only joins of about one pair an outer
/// row gain. The grid takes shapes as independent within a cell, so it
/// underestimates tables that share their positions: 0.92 pairs where
/// 4.0 were made on `bench/geometry_join/large.rs`. A quarter of a pair
/// estimated stands for about one made.
const PAIRED_MOST: f64 = 0.25;

impl PhysicalPlanner<'_> {
    /// The outer table's R-tree, if `choice`, a probe of `binding`'s
    /// inner column by the rows of `outer`, would cost more than a
    /// traversal of both trees; `same` if both columns have one SRID.
    pub(super) fn paired(&self, outer: &LogicalPlan, binding: &Binding, choice: &Choice, same: bool) -> Option<Box<Paired>> {
        let operation = binding.function.0.spatial_operation();
        let meets = matches!(operation, Some(SpatialOperation::Intersects | SpatialOperation::Meets));
        if !meets || !same || !matches!(binding.beside, super::Besides::Nothing) {
            return None;
        }
        let read = read_table(outer)?;
        self.db.index_of(&choice.read.index).ok()?.envelopes()?;
        let ix = self.db.catalog.indexes(read.table).into_iter().find(|ix| self.traversable(ix, binding.outer))?;
        let probed = self.db.catalog.indexes(&choice.read.table).into_iter().find(|p| p.name == choice.read.index)?;
        let (outer_stats, inner_stats) = (self.statistics(read.table)?, self.statistics(&choice.read.table)?);
        let (from, to) = (ix.columns.first().copied()?, probed.columns.first().copied()?);
        let each = cost_of(self.db.storage_of(&choice.read.table).ok()?.row_count())
            * (1.0_f64 - inner_stats.nulls(to).unwrap_or(0.0_f64))
            * outer_stats.envelopes(from)?.joined(inner_stats.envelopes(to)?);
        if each > PAIRED_MOST {
            return None;
        }
        (self.traversal_cost(&read, choice)? < self.probes_cost(outer, &read, choice)?).then(|| {
            Box::new(Paired {
                table: read.table.clone(),
                index: ix.name.clone().into(),
                filter: read.filter.cloned(),
                through: read.through.as_ref().map(|(columns, _)| columns.clone()),
                columns: read.needed.to_vec(),
                located: read.located,
            })
        })
    }

    /// Whether `ix` is a built R-tree of `column` alone that keeps its
    /// keys as a tree of envelopes.
    fn traversable(&self, ix: &IndexDesc, column: usize) -> bool {
        let built = !self.db.building.contains(&ix.name) && !ix.is_collated() && ix.columns.as_slice() == [column];
        built && self.db.index_of(&ix.name).ok().and_then(|found| found.envelopes()).is_some()
    }

    /// The probes' descents and the outer operand's scan, which the
    /// traversal saves, as the pairs' fetches of the inner table are the
    /// same either way.
    fn probes_cost(&self, outer: &LogicalPlan, read: &Read<'_>, choice: &Choice) -> Option<f64> {
        let model = self.weights();
        let inner = cost_of(self.db.storage_of(&choice.read.table).ok()?.row_count());
        let scan = self.db.storage_of(read.table).ok()?.scan_cost(read.needed);
        Some(self.rows_of(outer) * model.index_descent * (inner + 1.0_f64).log2() + scan)
    }

    /// Every entry of both trees compared, the outer rows of the pairs
    /// fetched, and of an outer join the outer table read again for the
    /// rows no pair kept.
    fn traversal_cost(&self, read: &Read<'_>, choice: &Choice) -> Option<f64> {
        let model = self.weights();
        let storage = self.db.storage_of(read.table).ok()?;
        let outer = cost_of(storage.row_count());
        let inner = cost_of(self.db.storage_of(&choice.read.table).ok()?.row_count());
        let pairs = outer * choice.read.each;
        let entries = (outer + inner) * model.sort_compare;
        let rest = if choice.preserve == Preserve::Inner { 0.0_f64 } else { storage.scan_cost(read.needed) };
        Some(entries + pairs.min(outer) * storage.fetch_cost() + rest)
    }
}
