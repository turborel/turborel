//! A filter's plan: part of its table's access path, a range join, a hash
//! join, or a filter over its input (`PhysicalPlanner::filter`).

use super::{Built, PhysicalPlanner, Plan};
use crate::logical::{Cond, LogicalPlan, SortKey, judged_by_sets, render_cond};

impl PhysicalPlanner<'_> {
    /// A filter directly on a table, or over a table's read and a guard
    /// (`physical/access/guarded.rs`), becomes part of the access path,
    /// told the columns the node above reads, `above`, if known; any other
    /// keeps the order of its input, its long IN lists judged by sets
    /// (`logical::judged_by_sets`).
    pub(super) fn filter(&self, cond: &Cond, input: &LogicalPlan, want: &[SortKey], above: Option<&[usize]>) -> Built {
        // A row goal is of the rows the filter keeps: its input is read
        // for as many more as it drops (`goals.rs`). A join divides its
        // own.
        let kept = |p: &PhysicalPlanner<'_>| p.per_row(p.estimating(|e| e.filter_card(cond, input)).rows, input);
        let goal = self.divided(|| kept(self));
        if let Some(read) = goal.filtered_read(cond, input, (want, above)) {
            return read;
        }
        if let Some(join) = self.range_join(cond, input) {
            return join;
        }
        if let Some(join) = self.hash_join(cond, input, want) {
            return join;
        }
        let est = self.estimating(|e| e.filter_card(cond, input)).rows;
        let b = goal.build_node(input, want);
        let cond = judged_by_sets(cond.clone());
        let detail = self.described(input, |names| render_cond(&cond, names));
        Built { plan: Plan::Filter { cond, detail, est, of_read: false, input: Box::new(b.plan) }, ordered: b.ordered }
    }
}
