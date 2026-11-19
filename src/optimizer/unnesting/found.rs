//! \[DESIGN\] The correlated subqueries of a node unnesting may take, and
//! how the node uses each (RULES.md D311).
//!
//! A subquery is evaluated for every row the node reads unless it stands
//! where a CASE may not reach: a WHEN's result, a WHEN's condition after
//! the first, or the ELSE [ISO/IEC 9075:1992 §6.9 GR2]. AND and OR
//! evaluate both operands (RULES.md D15), as a call does its arguments.
//! A subquery that may go unevaluated is conditional: unnesting computes
//! it for every row's binding, so it may only be one that cannot raise an
//! error.

use std::sync::Arc;

use crate::logical::{Cond, Expr, LogicalPlan, Quantifier, Subquery, split_and};
use crate::value::CompOp;

/// The node a rewrite starts at: a filter of its input's rows, or a
/// projection of them.
#[derive(Clone, Copy)]
pub(super) enum Node<'a> {
    /// WHERE or HAVING: the condition and the rows.
    Filter(&'a Cond, &'a LogicalPlan),
    /// A select list: the expressions and the rows.
    Project(&'a [Expr], &'a LogicalPlan),
}

impl<'a> Node<'a> {
    /// The node `plan` is, if unnesting starts at one of its kind.
    pub(super) fn of(plan: &'a LogicalPlan) -> Option<Node<'a>> {
        match plan {
            LogicalPlan::Filter { cond, input } => Some(Node::Filter(cond, input)),
            LogicalPlan::Project { exprs, input } => Some(Node::Project(exprs, input)),
            LogicalPlan::Scan { .. }
            | LogicalPlan::Product { .. }
            | LogicalPlan::Aggregate { .. }
            | LogicalPlan::Distinct { .. }
            | LogicalPlan::OuterJoin { .. }
            | LogicalPlan::SetOp { .. }
            | LogicalPlan::System { .. }
            | LogicalPlan::Values { .. }
            | LogicalPlan::Sort { .. }
            | LogicalPlan::Limit { .. }
            | LogicalPlan::Recursive { .. }
            | LogicalPlan::WorkTable { .. }
            | LogicalPlan::Transition { .. }
            | LogicalPlan::Unnest(_)
            | LogicalPlan::Window(_)
            | LogicalPlan::Recognize(_)
            | LogicalPlan::Polymorphic(_) => None,
        }
    }

    /// The rows it reads.
    pub(super) fn input(&self) -> &'a LogicalPlan {
        match self {
            Node::Filter(_, input) | Node::Project(_, input) => input,
        }
    }
}

/// A correlated subquery of a node.
pub(super) struct Found {
    /// The subquery.
    pub(super) sub: Arc<Subquery>,
    /// Whether every place it stands in may go unevaluated.
    pub(super) conditional: bool,
}

/// How the node uses a subquery at one place.
pub(super) enum Reference {
    /// EXISTS.
    Exists,
    /// A scalar or row subquery's column, or a comparison with its one row
    /// [X3.135-1986 §5.11]: of degree one the column; of a comparison the
    /// values compared and the operator.
    Value(Option<(Vec<Expr>, CompOp)>),
    /// A quantified comparison [X3.135-1986 §5.16]: the values compared,
    /// the operator, ALL rather than SOME, and whether it is a conjunct of
    /// a WHERE or HAVING condition, where unknown and false are alike.
    Quantified(Vec<Expr>, CompOp, bool, bool),
    /// UNIQUE or MATCH, which unnesting leaves alone.
    Other,
}

/// The correlated subqueries of `node`, each once, in the order met.
pub(super) fn candidates(node: &Node<'_>) -> Vec<Found> {
    let mut out = Vec::new();
    match node {
        Node::Filter(cond, _) => in_cond(cond, false, &mut out),
        Node::Project(exprs, _) => exprs.iter().for_each(|e| in_expr(e, false, &mut out)),
    }
    out
}

/// Notes `s`, if correlated, as met at a place `conditional` or not.
fn note(s: &Arc<Subquery>, conditional: bool, out: &mut Vec<Found>) {
    if s.outer.is_empty() {
        return;
    }
    match out.iter_mut().find(|f| f.sub.id() == s.id()) {
        Some(f) => f.conditional = f.conditional && conditional,
        None => out.push(Found { sub: Arc::clone(s), conditional }),
    }
}

/// The subqueries of `c`.
fn in_cond(c: &Cond, conditional: bool, out: &mut Vec<Found>) {
    match c {
        Cond::And(a, b) | Cond::Or(a, b) => {
            in_cond(a, conditional, out);
            in_cond(b, conditional, out);
        }
        Cond::Not(a) | Cond::Is(a, ..) => in_cond(a, conditional, out),
        Cond::Exists(s) | Cond::Unique(s, _) | Cond::Quantified(.., s) | Cond::Match { subquery: s, .. } => {
            note(s, conditional, out);
            c.for_each_expr(&mut |e| in_expr(e, conditional, out));
        }
        Cond::Const(_) | Cond::Compare(..) | Cond::Distinct(..) | Cond::IsNull(..) | Cond::Like(..) | Cond::In(..) | Cond::Value(_) => {
            c.for_each_expr(&mut |e| in_expr(e, conditional, out));
        }
    }
}

/// The subqueries of `e`.
fn in_expr(e: &Expr, conditional: bool, out: &mut Vec<Found>) {
    match e {
        Expr::Subquery(s, _) => {
            note(s, conditional, out);
            s.outer.iter().for_each(|o| in_expr(o, conditional, out));
        }
        Expr::Case(whens, otherwise) => {
            for (n, (c, result)) in whens.iter().enumerate() {
                in_cond(c, conditional || n > 0, out);
                in_expr(result, true, out);
            }
            in_expr(otherwise, true, out);
        }
        Expr::Truth(c) => in_cond(c, conditional, out),
        Expr::Column(_)
        | Expr::Const(_)
        | Expr::Neg(_)
        | Expr::Arith(..)
        | Expr::Call(..)
        | Expr::Outer(..)
        | Expr::Dynamic(_)
        | Expr::Cast(..)
        | Expr::Domain(..)
        | Expr::SortKey(..)
        | Expr::Warned(..)
        | Expr::NextValue(_)
        | Expr::Invoked(_) => e.for_each_child(&mut |child| in_expr(child, conditional, out)),
    }
}

/// How `node` uses the subquery `id`, at each place it stands.
pub(super) fn references(node: &Node<'_>, id: u64) -> Vec<Reference> {
    let mut out = Vec::new();
    match node {
        Node::Filter(cond, _) => {
            let mut conjuncts = Vec::new();
            split_and((*cond).clone(), &mut conjuncts);
            for c in &conjuncts {
                cond_references(c, (id, true), &mut out);
            }
        }
        Node::Project(exprs, _) => exprs.iter().for_each(|e| expr_references(e, id, &mut out)),
    }
    out
}

/// The references of `c` to `id`; `top` if `c` is a conjunct of a WHERE
/// or HAVING condition.
fn cond_references(c: &Cond, (id, top): (u64, bool), out: &mut Vec<Reference>) {
    match c {
        Cond::And(a, b) | Cond::Or(a, b) => {
            cond_references(a, (id, false), out);
            cond_references(b, (id, false), out);
            return;
        }
        Cond::Not(a) | Cond::Is(a, ..) => {
            cond_references(a, (id, false), out);
            return;
        }
        Cond::Exists(s) => {
            if s.id() == id {
                out.push(Reference::Exists);
            }
        }
        Cond::Quantified(xs, op, q, s) => {
            if s.id() == id {
                out.push(quantified(xs, *op, *q, top));
            }
        }
        Cond::Unique(s, _) | Cond::Match { subquery: s, .. } => {
            if s.id() == id {
                out.push(Reference::Other);
            }
        }
        Cond::Const(_) | Cond::Compare(..) | Cond::Distinct(..) | Cond::IsNull(..) | Cond::Like(..) | Cond::In(..) | Cond::Value(_) => {}
    }
    c.for_each_expr(&mut |e| expr_references(e, id, out));
}

/// The reference a comparison of `xs` by `op`, quantified by `q`, is.
fn quantified(xs: &[Expr], op: CompOp, q: Quantifier, top: bool) -> Reference {
    match q {
        Quantifier::One => Reference::Value(Some((xs.to_vec(), op))),
        Quantifier::All => Reference::Quantified(xs.to_vec(), op, true, top),
        Quantifier::Some => Reference::Quantified(xs.to_vec(), op, false, top),
    }
}

/// The references of `e` to `id`.
fn expr_references(e: &Expr, id: u64, out: &mut Vec<Reference>) {
    match e {
        Expr::Subquery(s, _) => {
            if s.id() == id {
                out.push(Reference::Value(None));
            }
        }
        Expr::Case(whens, otherwise) => {
            for (c, result) in whens {
                cond_references(c, (id, false), out);
                expr_references(result, id, out);
            }
            expr_references(otherwise, id, out);
            return;
        }
        Expr::Truth(c) => {
            cond_references(c, (id, false), out);
            return;
        }
        Expr::Column(_)
        | Expr::Const(_)
        | Expr::Neg(_)
        | Expr::Arith(..)
        | Expr::Call(..)
        | Expr::Outer(..)
        | Expr::Dynamic(_)
        | Expr::Cast(..)
        | Expr::Domain(..)
        | Expr::SortKey(..)
        | Expr::Warned(..)
        | Expr::NextValue(_)
        | Expr::Invoked(_) => {}
    }
    e.for_each_child(&mut |child| expr_references(child, id, out));
}
