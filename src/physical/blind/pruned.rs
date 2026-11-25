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
//! columns that gives that column null (`exec/batch/picks.rs`), or, if it
//! is a projection of columns, gives null itself, so the join or the sort
//! carries a vector of nulls in its place, which costs nothing to copy.
//! The plan is pruned in place, so a deep plan's walk holds little on the
//! stack. Every column keeps its place, so nothing above is
//! renumbered, and the plan's own columns are as they were. EXPLAIN shows
//! each such projection, the columns kept by name and the others as NULL
//! (`pruned/named.rs`).

mod joins;
mod named;
mod single;

use super::super::{PhysicalPlanner, Plan};
use super::width::Sized;
use single::scan_pruned;

use crate::logical::Expr;
use crate::value::Value;

/// The columns of an operator's rows that may hold a value, the others
/// null; none if not known here, so any may.
type Live = Option<Vec<usize>>;

/// A plan of no rows, left in a plan's place while it is moved.
fn placeholder() -> Plan {
    Plan::Values { rows: Vec::new(), detail: String::new() }
}

impl PhysicalPlanner<'_> {
    /// `plan`, each operator carrying only the columns read above it, its
    /// own columns as they were.
    pub(in crate::physical) fn pruned(&self, mut plan: Plan) -> Plan {
        self.prune(&mut plan);
        plan
    }

    /// Prunes `plan`, all of whose columns are read, its widths found once
    /// for the walk.
    fn prune(&self, plan: &mut Plan) {
        let sizes = self.sized(plan);
        // A grouping reads what its groups and its set functions read alone.
        if matches!(plan, Plan::Aggregate { .. }) {
            self.aggregate_pruned(plan, sizes.kid(0));
            return;
        }
        let Some(width) = sizes.width else {
            self.read_whole(plan);
            return;
        };
        self.pruned_in(plan, &sizes, &(0..width).collect::<Vec<_>>());
    }

    /// Prunes `plan`, of the widths `sizes`, of whose columns those above
    /// read `needed`: the columns of its rows that may hold a value.
    fn pruned_in(&self, plan: &mut Plan, sizes: &Sized, needed: &[usize]) -> Live {
        match plan {
            Plan::SeqScan { .. } => scan_pruned(plan, needed),
            Plan::Filter { .. } => self.filter_pruned(plan, sizes.kid(0), needed),
            Plan::Project { .. } => self.project_pruned(plan, sizes.kid(0), needed),
            Plan::Sort { keys, input, .. } => {
                let reads: Vec<usize> = needed.iter().copied().chain(keys.iter().map(|k| k.column)).collect();
                self.operand_pruned(input, sizes.kid(0), &reads)
            }
            Plan::Limit { input, .. } => self.pruned_in(input, sizes.kid(0), needed),
            Plan::Aggregate { .. } => self.aggregate_pruned(plan, sizes.kid(0)),
            Plan::HashJoin { .. } | Plan::Product { .. } => self.join_pruned(plan, sizes, needed),
            Plan::OuterJoin { .. } => self.outer_pruned(plan, sizes, needed),
            Plan::SemiJoin(semi) => self.semi_pruned(semi, sizes, needed),
            Plan::Nearest(_)
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
            | Plan::Counted(_) => self.read_whole(plan),
        }
    }

    /// Prunes `plan`, an operator that reads every column of its inputs,
    /// or of a kind not pruned here: each input as a plan of its own.
    fn read_whole(&self, plan: &mut Plan) -> Live {
        plan.each_input(&mut |input| self.prune(input));
        None
    }

    /// Prunes `operand`, of the widths `sizes`, an operand of a join or a
    /// sort's input, of whose columns it and those above read `reads`, and
    /// gives null for each column it gives that none of them reads: a
    /// projection of columns itself, any other under one. Its columns are
    /// named for EXPLAIN alone.
    fn operand_pruned(&self, operand: &mut Plan, sizes: &Sized, reads: &[usize]) -> Live {
        let live = self.pruned_in(operand, sizes, reads)?;
        if live.iter().all(|c| reads.contains(c)) {
            return Some(live);
        }
        let Some(width) = sizes.width else { return Some(live) };
        let shown = |c: usize, name: String| if reads.contains(&c) { name } else { "NULL".to_owned() };
        let detail = if self.shown {
            // A projection's own names, as it shows them, if one for each.
            let own: Option<Vec<String>> = if let Plan::Project { exprs, detail, .. } = &*operand {
                let pieces: Vec<String> = detail.split(", ").map(str::to_owned).collect();
                (pieces.len() == exprs.len()).then_some(pieces)
            } else {
                None
            };
            let names = own.unwrap_or_else(|| self.names(operand, width));
            names.into_iter().enumerate().map(|(c, name)| shown(c, name)).collect::<Vec<_>>().join(", ")
        } else {
            String::new()
        };
        if let Plan::Project { exprs, detail: text, .. } = operand
            && exprs.iter().all(moved)
        {
            for (c, e) in exprs.iter_mut().enumerate() {
                if !reads.contains(&c) {
                    *e = Expr::Const(Value::Null);
                }
            }
            *text = detail;
        } else {
            let exprs = (0..width).map(|c| if reads.contains(&c) { Expr::Column(c) } else { Expr::Const(Value::Null) }).collect();
            let input = Box::new(std::mem::replace(operand, placeholder()));
            *operand = Plan::Project { exprs, detail, input };
        }
        Some(live.into_iter().filter(|c| reads.contains(c)).collect())
    }
}

/// Whether `e` is a column moved or the null value.
fn moved(e: &Expr) -> bool {
    if let Expr::Const(v) = e { v.is_null() } else { matches!(e, Expr::Column(_)) }
}

#[cfg(test)]
mod tests;
