//! \[DESIGN\] (RULES.md D1167) Each operator carries only the columns read
//! above it (Abadi, Myers, DeWitt and Madden, "Materialization Strategies
//! in a Column-Oriented DBMS", ICDE 2007, on late materialization).
//!
//! A scan read every column its statement uses, and each operator carried
//! every column its input gave, so a column read only below, by a filter
//! or as a join's key, was copied into each row of every join above it.
//! A plan is now walked from the top with the columns of each operator's
//! rows read above it, its own columns all. A scan reads those and the
//! ones it is ordered by alone. A filter reads its condition's, a sort its
//! keys', a grouping its groups' and its set functions', and a join its
//! keys' and its conditions'. A projection's column moved is read only if
//! one above reads it; any other expression reads its columns whether or
//! not it is read, so no error it raises is lost. An operator with a
//! subquery is left as it is, with all it reads, as the subquery may read
//! the row, and so is one of any other kind.
//!
//! An operand of a join, or the input of a sort, that gives a column
//! neither it nor one above reads is put under a projection of its
//! columns that gives that column null (`exec/batch/picks.rs`), so the
//! join or the sort carries a vector of nulls in its place, which costs
//! nothing to copy. Every column keeps its place, so nothing above is
//! renumbered, and the plan's own columns are as they were. EXPLAIN shows
//! each such projection, the columns kept by name and the others as NULL
//! (`pruned/named.rs`).

mod joins;
mod named;
mod single;

use super::super::{PhysicalPlanner, Plan};
use single::scan_pruned;

use crate::logical::Expr;
use crate::value::Value;

/// The columns of an operator's rows that may hold a value, the others
/// null; none if not known here, so any may.
type Live = Option<Vec<usize>>;

impl PhysicalPlanner<'_> {
    /// `plan`, each operator carrying only the columns read above it, its
    /// own columns as they were.
    pub(in crate::physical) fn pruned(&self, plan: Plan) -> Plan {
        let Some(width) = self.sized(&plan).width else { return self.whole(plan).0 };
        self.pruned_in(plan, &(0..width).collect::<Vec<_>>()).0
    }

    /// `plan`, of whose columns those above read `needed`, pruned, and the
    /// columns of its rows that may hold a value.
    fn pruned_in(&self, plan: Plan, needed: &[usize]) -> (Plan, Live) {
        match plan {
            Plan::SeqScan { .. } => scan_pruned(plan, needed),
            Plan::Filter { .. } => self.filter_pruned(plan, needed),
            Plan::Project { .. } => self.project_pruned(plan, needed),
            Plan::Sort { keys, detail, input, most } => {
                let reads: Vec<usize> = needed.iter().copied().chain(keys.iter().map(|k| k.column)).collect();
                let (input, live) = self.operand_pruned(*input, &reads);
                (Plan::Sort { keys, detail, input: Box::new(input), most }, live)
            }
            Plan::Limit { limit, input, est } => {
                let (input, live) = self.pruned_in(*input, needed);
                (Plan::Limit { limit, input: Box::new(input), est }, live)
            }
            Plan::Aggregate { .. } => self.aggregate_pruned(plan),
            Plan::HashJoin { left, right, keys, detail, est } => self.hash_pruned((*left, *right), (keys, detail, est), needed),
            Plan::Product { left, right, est } => self.product_pruned((*left, *right), est, needed),
            Plan::OuterJoin { .. } => self.outer_pruned(plan, needed),
            Plan::SemiJoin(semi) => self.semi_pruned(*semi, needed),
            other @ (Plan::Nearest(_)
            | Plan::IndexScan { .. }
            | Plan::IndexJoin { .. }
            | Plan::MergeJoin(_)
            | Plan::RangeJoin(_)
            | Plan::MarkJoin(_)
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
            | Plan::Counted(_)) => self.whole(other),
        }
    }

    /// `plan`, an operator that reads every column of its inputs, or of a
    /// kind not pruned here, each input pruned as a plan of its own.
    fn whole(&self, mut plan: Plan) -> (Plan, Live) {
        plan.each_input(&mut |input| {
            let taken = std::mem::replace(input, Plan::Values { rows: Vec::new(), detail: String::new() });
            *input = self.pruned(taken);
        });
        (plan, None)
    }

    /// `operand`, an operand of a join or a sort's input, of whose columns
    /// it and those above read `reads`, pruned, and put under a projection
    /// giving null for each column it gives that none of them reads.
    fn operand_pruned(&self, operand: Plan, reads: &[usize]) -> (Plan, Live) {
        let (operand, live) = self.pruned_in(operand, reads);
        let Some(width) = self.sized(&operand).width else { return (operand, live) };
        let live = live.unwrap_or_else(|| (0..width).collect());
        if live.iter().all(|c| reads.contains(c)) {
            return (operand, Some(live));
        }
        let names = self.names(&operand, width);
        let (mut exprs, mut shown) = (Vec::with_capacity(width), Vec::with_capacity(width));
        for (c, name) in names.into_iter().enumerate() {
            let kept = reads.contains(&c);
            exprs.push(if kept { Expr::Column(c) } else { Expr::Const(Value::Null) });
            shown.push(if kept { name } else { "NULL".to_owned() });
        }
        let kept = live.into_iter().filter(|c| reads.contains(c)).collect();
        (Plan::Project { exprs, detail: shown.join(", "), input: Box::new(operand) }, Some(kept))
    }
}

#[cfg(test)]
mod tests;
