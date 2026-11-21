//! The width of a physical plan's rows, of the operators the reduction
//! walks through (`physical/blind.rs`).

use super::super::{PhysicalPlanner, Plan, Preserved};

impl PhysicalPlanner<'_> {
    /// The columns of each row of `plan`, if its operator is one whose
    /// width is known here: a scan's, its table's and its row id's; a
    /// join's, its operands'; a semi join's, its left's.
    pub(super) fn width(&self, plan: &Plan) -> Option<usize> {
        let table = |name| self.db.catalog.table(name).map(|t| t.columns.len());
        match plan {
            Plan::SeqScan { table: name, located, .. } | Plan::IndexScan { table: name, located, .. } => {
                table(name).map(|w| w.saturating_add(usize::from(*located)))
            }
            Plan::Filter { input, .. } => self.width(input),
            Plan::Counted(c) => self.width(&c.input),
            Plan::Project { exprs, .. } => Some(exprs.len()),
            Plan::HashJoin { left, right, .. } | Plan::Product { left, right, .. } => {
                Some(self.width(left)?.saturating_add(self.width(right)?))
            }
            Plan::IndexJoin { outer, left, width, .. } => {
                let alone = matches!(outer, Preserved::Semi | Preserved::Anti);
                Some(self.width(left)?.saturating_add(if alone { 0 } else { *width }))
            }
            Plan::SemiJoin(s) => Some(s.widths.0),
            Plan::Nearest(_)
            | Plan::OuterJoin { .. }
            | Plan::MergeJoin(_)
            | Plan::RangeJoin(_)
            | Plan::Aggregate { .. }
            | Plan::Distinct { .. }
            | Plan::SetOp { .. }
            | Plan::System { .. }
            | Plan::Values { .. }
            | Plan::Append { .. }
            | Plan::MergeAppend { .. }
            | Plan::Sort { .. }
            | Plan::Limit { .. }
            | Plan::Recursive { .. }
            | Plan::WorkTable { .. }
            | Plan::Transition { .. }
            | Plan::Unnest(_)
            | Plan::Window { .. }
            | Plan::Recognize { .. }
            | Plan::Polymorphic(_)
            | Plan::Parallel(_) => None,
        }
    }
}
