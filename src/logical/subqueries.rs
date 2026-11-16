//! The subqueries of a plan's expressions and conditions, and the base
//! tables they read. The body of an SQL-invoked function invoked is no
//! subquery of the plan, as its outer expressions are over the row of its
//! parameters (`logical/invoked.rs`), but the tables it reads are read by
//! the plan: so the walks that find those reach it too.

use super::{Cond, Expr, LogicalPlan, Subquery};
use crate::catalog::QualifiedName;

/// Adds to `out` the subqueries of `e`, those of its conditions among
/// them, not those nested in them; and, if `bodies`, the bodies of the
/// functions it invokes.
fn expr_subqueries<'e>(e: &'e Expr, out: &mut Vec<&'e Subquery>, bodies: bool) {
    match e {
        Expr::Subquery(s, _) => out.push(s),
        Expr::Case(whens, _) => out.extend(whens.iter().flat_map(|(c, _)| cond_subqueries(c, bodies))),
        Expr::Truth(c) => out.extend(cond_subqueries(c, bodies)),
        // A domain's constraints are run for the value CAST, and read their tables.
        Expr::Domain(_, d) => d.checks.iter().for_each(|(_, c)| out.extend(cond_subqueries(c, bodies))),
        Expr::Invoked(i) => {
            if let (true, Some(body)) = (bodies, &i.body) {
                out.push(body);
            }
        }
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
    e.for_each_child(&mut |child| expr_subqueries(child, out, bodies));
}

/// The subqueries of `c`, those of its expressions among them, not those
/// nested in them; and, if `bodies`, the bodies of the functions it
/// invokes.
fn cond_subqueries(c: &Cond, bodies: bool) -> Vec<&Subquery> {
    let mut out = c.subqueries();
    c.for_each_expr(&mut |e| expr_subqueries(e, &mut out, bodies));
    out
}

impl Cond {
    /// Its subqueries, those of its expressions among them, not those
    /// nested in them.
    pub fn all_subqueries(&self) -> Vec<&Subquery> {
        cond_subqueries(self, false)
    }

    /// Whether it has a subquery, among its expressions' too.
    pub fn has_subquery(&self) -> bool {
        !cond_subqueries(self, false).is_empty()
    }

    /// Every base table the subqueries of this condition read, none if it
    /// has none.
    pub fn tables(&self) -> Vec<QualifiedName> {
        let mut out = Vec::new();
        for s in cond_subqueries(self, true) {
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
        expr_subqueries(self, &mut out, false);
        out
    }

    /// Every base table the subqueries of this expression read, none if it
    /// has none.
    pub fn tables(&self) -> Vec<QualifiedName> {
        let (mut subqueries, mut out) = (Vec::new(), Vec::new());
        expr_subqueries(self, &mut subqueries, true);
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
        for s in self.subqueries_of(true) {
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
        self.subqueries_of(false)
    }

    /// Those, and, if `bodies`, the bodies of the functions this node's
    /// own expressions and conditions invoke: what reads the tables it
    /// reads beside its inputs.
    pub(super) fn subqueries_of(&self, bodies: bool) -> Vec<&Subquery> {
        let mut out = Vec::new();
        match self {
            LogicalPlan::Filter { cond, .. } | LogicalPlan::OuterJoin { cond, .. } => out = cond_subqueries(cond, bodies),
            LogicalPlan::Project { exprs, .. } => exprs.iter().for_each(|e| expr_subqueries(e, &mut out, bodies)),
            LogicalPlan::Aggregate { aggs, .. } => {
                for a in aggs {
                    a.exprs().into_iter().for_each(|e| expr_subqueries(e, &mut out, bodies));
                }
            }
            LogicalPlan::Values { rows, .. } => rows.iter().flatten().for_each(|e| expr_subqueries(e, &mut out, bodies)),
            LogicalPlan::Unnest(u) => {
                u.collections.iter().for_each(|e| expr_subqueries(e, &mut out, bodies));
                out.extend(u.outer.iter().flat_map(|c| cond_subqueries(c, bodies)));
            }
            LogicalPlan::Window(w) => {
                w.exprs().for_each(|e| expr_subqueries(e, &mut out, bodies));
                out.extend(w.conds().flat_map(|c| cond_subqueries(c, bodies)));
            }
            LogicalPlan::Recognize(r) => {
                r.exprs().for_each(|e| expr_subqueries(e, &mut out, bodies));
                out.extend(r.conds().flat_map(|c| cond_subqueries(c, bodies)));
            }
            LogicalPlan::Limit { limit, .. } => limit.exprs().for_each(|e| expr_subqueries(e, &mut out, bodies)),
            LogicalPlan::Scan { .. }
            | LogicalPlan::System { .. }
            | LogicalPlan::Product { .. }
            | LogicalPlan::Distinct { .. }
            | LogicalPlan::SetOp { .. }
            | LogicalPlan::Sort { .. }
            | LogicalPlan::Recursive { .. }
            | LogicalPlan::WorkTable { .. }
            | LogicalPlan::Transition { .. } => {}
        }
        out
    }
}
