//! The subqueries of a plan's expressions and conditions, and the base
//! tables they read.

use super::{Cond, Expr, LogicalPlan, Subquery};
use crate::catalog::QualifiedName;

/// Adds to `out` the subqueries of `e`, those of its conditions among
/// them, not those nested in them.
fn expr_subqueries<'e>(e: &'e Expr, out: &mut Vec<&'e Subquery>) {
    match e {
        Expr::Subquery(s, _) => out.push(s),
        Expr::Case(whens, _) => out.extend(whens.iter().flat_map(|(c, _)| c.subqueries())),
        Expr::Truth(c) => out.extend(c.subqueries()),
        // A domain's constraints are run for the value CAST, and read their tables.
        Expr::Domain(_, d) => d.checks.iter().for_each(|(_, c)| out.extend(cond_subqueries(c))),
        Expr::Column(_)
        | Expr::Const(_)
        | Expr::Neg(_)
        | Expr::Arith(..)
        | Expr::Call(..)
        | Expr::Outer(..)
        | Expr::Dynamic(_)
        | Expr::Cast(..)
        | Expr::SortKey(..)
        | Expr::NextValue(_) => {}
    }
    e.for_each_child(&mut |child| expr_subqueries(child, out));
}

/// The subqueries of `c`, those of its expressions among them, not those
/// nested in them.
fn cond_subqueries(c: &Cond) -> Vec<&Subquery> {
    let mut out = c.subqueries();
    c.for_each_expr(&mut |e| expr_subqueries(e, &mut out));
    out
}

impl Cond {
    /// Its subqueries, those of its expressions among them, not those
    /// nested in them.
    pub fn all_subqueries(&self) -> Vec<&Subquery> {
        cond_subqueries(self)
    }

    /// Whether it has a subquery, among its expressions' too.
    pub fn has_subquery(&self) -> bool {
        !cond_subqueries(self).is_empty()
    }

    /// Every base table the subqueries of this condition read, none if it
    /// has none.
    pub fn tables(&self) -> Vec<QualifiedName> {
        let mut out = Vec::new();
        for s in cond_subqueries(self) {
            s.plan.collect_tables(&mut out);
        }
        out
    }
}

impl Expr {
    /// Its subqueries, those of its conditions among them, not those
    /// nested in them.
    pub fn all_subqueries(&self) -> Vec<&Subquery> {
        let mut out = Vec::new();
        expr_subqueries(self, &mut out);
        out
    }

    /// Every base table the subqueries of this expression read, none if it
    /// has none.
    pub fn tables(&self) -> Vec<QualifiedName> {
        let (mut subqueries, mut out) = (Vec::new(), Vec::new());
        expr_subqueries(self, &mut subqueries);
        for s in subqueries {
            s.plan.collect_tables(&mut out);
        }
        out
    }
}

impl LogicalPlan {
    /// Every base table the subqueries of this plan read, at any node.
    pub fn subquery_tables(&self) -> Vec<QualifiedName> {
        let mut out = Vec::new();
        for s in self.subqueries() {
            s.plan.collect_tables(&mut out);
        }
        for input in self.inputs() {
            out.extend(input.subquery_tables());
        }
        out
    }

    /// The subqueries of this node's own expressions and conditions, not
    /// those of its inputs.
    pub fn subqueries(&self) -> Vec<&Subquery> {
        let mut out = Vec::new();
        match self {
            LogicalPlan::Filter { cond, .. } | LogicalPlan::OuterJoin { cond, .. } => out = cond_subqueries(cond),
            LogicalPlan::Project { exprs, .. } => exprs.iter().for_each(|e| expr_subqueries(e, &mut out)),
            LogicalPlan::Aggregate { aggs, .. } => {
                for a in aggs {
                    a.exprs().into_iter().for_each(|e| expr_subqueries(e, &mut out));
                }
            }
            LogicalPlan::Values { rows, .. } => rows.iter().flatten().for_each(|e| expr_subqueries(e, &mut out)),
            LogicalPlan::Unnest(u) => {
                u.collections.iter().for_each(|e| expr_subqueries(e, &mut out));
                out.extend(u.outer.iter().flat_map(cond_subqueries));
            }
            LogicalPlan::Window(w) => {
                w.exprs().for_each(|e| expr_subqueries(e, &mut out));
                out.extend(w.conds().flat_map(cond_subqueries));
            }
            LogicalPlan::Recognize(r) => {
                r.exprs().for_each(|e| expr_subqueries(e, &mut out));
                out.extend(r.conds().flat_map(cond_subqueries));
            }
            LogicalPlan::Limit { limit, .. } => limit.exprs().for_each(|e| expr_subqueries(e, &mut out)),
            LogicalPlan::Scan { .. }
            | LogicalPlan::System { .. }
            | LogicalPlan::Product { .. }
            | LogicalPlan::Distinct { .. }
            | LogicalPlan::SetOp { .. }
            | LogicalPlan::Sort { .. }
            | LogicalPlan::Recursive { .. }
            | LogicalPlan::WorkTable { .. } => {}
        }
        out
    }
}
