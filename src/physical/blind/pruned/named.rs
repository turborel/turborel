//! The names EXPLAIN gives the columns a pruning projection keeps
//! (`blind/pruned.rs`, RULES.md D1167): a table's column by its table and
//! its name, through the operators that move it, and any other by its
//! place.

use super::super::super::{PhysicalPlanner, Plan};
use crate::catalog::Catalog;
use crate::logical::Expr;

impl PhysicalPlanner<'_> {
    /// The names of the `width` columns of `plan`'s rows.
    pub(super) fn names(&self, plan: &Plan, width: usize) -> Vec<String> {
        let mut out = self.named(plan);
        out.truncate(width);
        while out.len() < width {
            out.push(format!("#{}", out.len()));
        }
        out
    }

    /// The names of the columns of `plan`'s rows that it is known to give.
    fn named(&self, plan: &Plan) -> Vec<String> {
        match plan {
            Plan::SeqScan { table, .. } | Plan::IndexScan { table, .. } => self
                .db
                .catalog
                .table(table)
                .map(|desc| desc.columns.iter().map(|c| format!("{}.{}", table.ident, c.name)).collect())
                .unwrap_or_default(),
            Plan::Filter { input, .. } | Plan::Sort { input, .. } | Plan::Limit { input, .. } => self.named(input),
            Plan::Project { exprs, input, .. } => {
                let below = self.named(input);
                let name = |(n, e): (usize, &Expr)| {
                    let moved = if let Expr::Column(c) = e { below.get(*c).cloned() } else { None };
                    moved.unwrap_or_else(|| format!("#{n}"))
                };
                exprs.iter().enumerate().map(name).collect()
            }
            Plan::HashJoin { left, right, .. } | Plan::Product { left, right, .. } | Plan::OuterJoin { left, right, .. } => {
                let Some(lw) = self.sized(left).width else { return Vec::new() };
                let mut out = self.names(left, lw);
                out.extend(self.named(right));
                out
            }
            Plan::SemiJoin(s) => self.named(&s.left),
            Plan::Nearest(_)
            | Plan::IndexJoin { .. }
            | Plan::MergeJoin(_)
            | Plan::RangeJoin(_)
            | Plan::MarkJoin(_)
            | Plan::Aggregate { .. }
            | Plan::Distinct { .. }
            | Plan::SetOp { .. }
            | Plan::System { .. }
            | Plan::Values { .. }
            | Plan::Append { .. }
            | Plan::MergeAppend { .. }
            | Plan::Recursive { .. }
            | Plan::WorkTable { .. }
            | Plan::Transition { .. }
            | Plan::Shared(_)
            | Plan::Unnest(_)
            | Plan::Window { .. }
            | Plan::Recognize { .. }
            | Plan::Polymorphic(_)
            | Plan::Parallel(_)
            | Plan::Transfer(_)
            | Plan::Counted(_) => Vec::new(),
        }
    }
}
