//! [ISO/IEC 9075-2:2023 §7.7] The plan of a row pattern recognition: its
//! input sorted by its partitioning, then its ordering.

use super::{Built, PhysicalPlanner, Plan};
use crate::logical::{LogicalPlan, Recognize, SortKey, render_keys};

impl PhysicalPlanner<'_> {
    /// The row pattern recognition `r`, its input sorted by its
    /// partitioning then its ordering, unless the input gives that order.
    /// Its rows are in the order of its partitions and matches, not one
    /// `want` asks for.
    pub(super) fn recognize(&self, r: &Recognize<LogicalPlan>) -> Built {
        let mut keys: Vec<SortKey> = r.partition.iter().map(|&c| SortKey::ascending(c)).collect();
        keys.extend(r.order.iter().copied());
        let b = self.build(&r.input, &keys);
        let detail = self.described(&r.input, |names| r.render(names));
        let input = if b.ordered {
            b.plan
        } else {
            let sorting = self.described(&r.input, |names| render_keys(&keys, names));
            Plan::Sort { keys: keys.clone(), detail: sorting, input: Box::new(b.plan), most: None }
        };
        Built::unordered(Plan::Recognize { recognize: Box::new(r.with_input(|_| ()).over(input)), detail })
    }
}
