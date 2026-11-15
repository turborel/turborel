//! [ISO/IEC 9075-2:2023 §4.26.2] The counterparts of a plan's columns in
//! the base tables it reads. A column is a counterpart of the column a
//! column reference that specifies it references, and so, in turn, of
//! that column's counterpart: a projection that is a column reference, a
//! grouping column, a column of either operand of a join, or a column a
//! filter, an ordering, a window or a collection derived table passes on.
//! Any other value, a set function's, a window function's, an element's,
//! or a column of a set operation, which is specified by no column
//! reference, has none. A view, bound into the plan of the query that
//! reads it, is passed through, so each column found is of a leaf
//! generally underlying table, a base table.
//!
//! What a view's column takes as its default is found from it [§11.5
//! GR3d] (`binder/counterparts.rs`).

use super::{Expr, LogicalPlan};
use crate::catalog::QualifiedName;

/// A column of a base table: the table, and the column's place in it.
pub type BaseColumn = (QualifiedName, usize);

impl LogicalPlan {
    /// Of each column of its output, in order, the column of a base table
    /// it is the counterpart of, if it is one's.
    pub fn counterparts(&self) -> Vec<Option<BaseColumn>> {
        match self {
            LogicalPlan::Scan { table, names, located, .. } => {
                // The row id a MERGE's target reads is no column of it.
                let columns = names.len().saturating_sub(usize::from(*located));
                let mut out: Vec<Option<BaseColumn>> = (0..columns).map(|k| Some((table.clone(), k))).collect();
                out.resize(names.len(), None);
                out
            }
            LogicalPlan::Filter { input, .. }
            | LogicalPlan::Distinct { input }
            | LogicalPlan::Sort { input, .. }
            | LogicalPlan::Limit { input, .. } => input.counterparts(),
            LogicalPlan::Product { left, right } | LogicalPlan::OuterJoin { left, right, .. } => {
                let mut out = left.counterparts();
                out.extend(right.counterparts());
                out
            }
            LogicalPlan::Aggregate { groups, aggs, input, .. } => {
                let inner = input.counterparts();
                let mut out: Vec<Option<BaseColumn>> = groups.iter().map(|&g| inner.get(g).cloned().flatten()).collect();
                out.resize(groups.len().saturating_add(aggs.len()), None);
                out
            }
            LogicalPlan::Project { exprs, input } => {
                let inner = input.counterparts();
                exprs.iter().map(|e| if let Expr::Column(k) = e { inner.get(*k).cloned().flatten() } else { None }).collect()
            }
            // Its input's columns first, then those it computes.
            LogicalPlan::Unnest(u) => self.passed(&u.input),
            LogicalPlan::Window(w) => self.passed(&w.input),
            LogicalPlan::Recognize(r) => self.passed(&r.input),
            LogicalPlan::SetOp { .. }
            | LogicalPlan::System { .. }
            | LogicalPlan::Values { .. }
            | LogicalPlan::Recursive { .. }
            | LogicalPlan::WorkTable { .. }
            | LogicalPlan::Transition { .. } => vec![None; self.width()],
        }
    }

    /// Its counterparts, of a node whose output is the columns of `input`
    /// followed by columns of its own, which have none.
    fn passed(&self, input: &LogicalPlan) -> Vec<Option<BaseColumn>> {
        let mut out = input.counterparts();
        out.resize(self.width(), None);
        out
    }
}

#[cfg(test)]
mod tests;
