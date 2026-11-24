//! \[DESIGN\] Unnesting correlated subqueries (RULES.md D310 to D319),
//! after Neumann and Kemper, "Unnesting Arbitrary Queries" (2015), and
//! Neumann, "Improving Unnesting of Complex Queries" (2025).
//!
//! A correlated subquery is "effectively executed for each row" its
//! predicate is applied to [X3.135-1986 §5.21 GR3, §5.23 GR4], which is
//! a dependent join of the node's rows with it. The rule makes that a join
//! of ordinary plans, so the subquery is computed once, not once for each
//! binding of its outer references.
//!
//! It takes a correlated subquery of a WHERE or HAVING condition or of a
//! select list (`found.rs`) and works in these steps.
//!
//! 1. Pulls its correlation up out of it, if every correlation is an
//!    equality of its columns with outer references and no part of it can
//!    raise an error, so the subquery is computed for every value of
//!    those columns (`simple.rs`, §3.1 and §4 of the 2015 paper).
//! 2. Else pushes the dependent join with D, the distinct bindings of the
//!    node's rows, down its plan until it reads no outer reference
//!    (`domain.rs`, §3.2).
//! 3. Joins the node's rows with a row for each binding, and reads that
//!    row where the subquery stood (`joined.rs`).
//!
//! A quantified comparison that is a conjunct of WHERE or HAVING is first
//! made EXISTS of the rows it holds of, or, of ALL, NOT EXISTS of those it
//! does not, as there unknown and false are alike [X3.135-1986 §5.16 GR2,
//! §5.21 GR2]. The rule runs before the others, which then move the
//! conditions of the plan it made as of any (2015 paper, §3.2).
//!
//! A subquery of EXISTS, NOT EXISTS or such a comparison is made a semi
//! join or an antijoin first, by a rule of its own (`semi.rs`), and one of
//! EXISTS or a quantified comparison elsewhere a mark join (`marked.rs`).
//!
//! Subqueries it leaves are run as before, for each binding
//! (`exec/subquery.rs`). Those are the subqueries of UNIQUE or MATCH, of
//! plans or outer rows that are possibly non-deterministic or change
//! data (`pure.rs`), of an operator it does not push D down, and
//! conditional ones that can raise.
//! The subqueries of a subquery are unnested once it is, on the rule's
//! next pass, top down, as the 2025 paper's §3 asks, so no domain is
//! pushed across another's dependent join.

mod calls;
mod domain;
mod found;
mod joined;
mod marked;
mod pure;
mod scalar;
mod semi;
mod simple;
mod typed;

pub(crate) use calls::Assigned;
pub use marked::{MARK_JOINS, MarkJoins};
pub(crate) use pure::expr_pure;
pub use semi::{SEMI_JOINS, SemiJoins};

use found::{Found, Node, Reference};

use super::{Rule, RuleContext};
use crate::logical::{Cond, Expr, LogicalPlan, Subquery, Truth, and_all, or_all, render_expr};
use crate::value::CompOp;

/// The rule's name, as EXPLAIN shows it and its setting names it.
pub const UNNEST_SUBQUERIES: &str = "unnest_subqueries";

/// Unnests the correlated subqueries of a filter or a projection, one at
/// a time (see the module's documentation).
pub struct UnnestSubqueries;

impl Rule for UnnestSubqueries {
    fn name(&self) -> &'static str {
        UNNEST_SUBQUERIES
    }
    fn apply(&self, plan: &LogicalPlan, ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        let node = Node::of(plan)?;
        found::candidates(&node).iter().find_map(|f| unnested(&node, f, ctx))
    }
}

/// How the rows a subquery gives for a binding are reduced to the one row
/// the node reads of them.
pub(super) enum How {
    /// Whether there is one, of EXISTS.
    Exists,
    /// The count of them and the first's values.
    Value,
    /// The counts of those a comparison of the values at the outer
    /// reference slots `slots` by `op` decides (false of ALL, true of
    /// SOME) and leaves unknown.
    Quantified {
        /// The comparison operator.
        op: CompOp,
        /// ALL rather than SOME.
        all: bool,
        /// The slots of the values compared.
        slots: Vec<usize>,
    },
}

/// The subquery a rewrite unnests.
pub(super) struct Target {
    /// The identity of the subquery the node uses, whose places are
    /// replaced.
    id: u64,
    /// The subquery unnested, which is it or what it was made, with the
    /// values it is compared with as outer references of its own.
    sub: Subquery,
    /// How its rows are reduced.
    how: How,
    /// Of a quantified comparison made EXISTS, whether NOT EXISTS.
    negated: bool,
    /// Whether the subquery's every row is read where it stands, so it
    /// raises each of its rows' errors (RULES.md D15).
    whole: bool,
}

/// A subquery unnested, which holds the rows of its bindings, its key
/// columns first; the node's rows' keys; and how keys match.
pub(super) struct Bound {
    /// The plan.
    plan: LogicalPlan,
    /// The keys, over the node's rows.
    keys: Vec<Expr>,
    /// Keys match as IS NOT DISTINCT FROM, rather than as =.
    not_distinct: bool,
    /// The value of each of its columns for a binding with no row, of a
    /// subquery made a grouping (`simple.rs`).
    defaults: Option<Vec<Expr>>,
    /// Whether it has one row at most of each binding, a grouping by its
    /// keys, so no count of them is needed.
    unique: bool,
}

/// The node `node` with the subquery `found` unnested, if it can be.
fn unnested(node: &Node<'_>, found: &Found, ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
    let functions = &ctx.db.functions;
    let target = target(node, found)?;
    // The bindings are grouped and hashed, so each must be of a plain type.
    if !target.sub.outer.iter().all(|o| typed::plain_key(o, node.input(), &*ctx.db.catalog)) {
        return None;
    }
    let pure = target.sub.outer.iter().all(expr_pure) && pure::plan_pure(&target.sub.plan);
    // A subquery that may go unevaluated is computed for every binding,
    // and every row of it read. EXISTS may stop at its deciding row and a
    // scalar subquery at its second, unless it is a grouping of one row,
    // while IN, SOME and ALL read every row (RULES.md D15).
    let outer_raises = found.sub.outer.iter().any(Expr::can_raise);
    let plan_raises = || pure::plan_raises(&found.sub.plan, ctx);
    let stops = !target.whole && plan_raises();
    if !pure || stops || (found.conditional && (outer_raises || plan_raises())) {
        return None;
    }
    let simple = match target.how {
        How::Quantified { .. } => None,
        How::Exists | How::Value => simple::pulled(&target.sub, ctx),
    };
    // EXISTS of a grouping of no grouping columns has a row of every
    // binding, which no default says.
    let simple = simple.filter(|b| b.defaults.is_none() || matches!(target.how, How::Value));
    let bound = simple.or_else(|| domain::pushed(&target.sub, node.input()))?;
    joined::rewritten(node, &target, bound, functions)
}

/// Whether `plan` is a grouping of no grouping columns, under projections,
/// which reads every row of its input to give its one row.
fn one_row(plan: &LogicalPlan) -> bool {
    match plan {
        LogicalPlan::Project { input, .. } => one_row(input),
        LogicalPlan::Aggregate { groups, .. } => groups.is_empty(),
        LogicalPlan::Scan { .. }
        | LogicalPlan::Filter { .. }
        | LogicalPlan::Product { .. }
        | LogicalPlan::Distinct { .. }
        | LogicalPlan::OuterJoin { .. }
        | LogicalPlan::SemiJoin { .. }
        | LogicalPlan::MarkJoin(_)
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
        | LogicalPlan::Polymorphic(_) => false,
    }
}

/// What `node` makes of the subquery `found`, or none if it uses it in ways
/// unnesting does not take.
fn target(node: &Node<'_>, found: &Found) -> Option<Target> {
    let refs = found::references(node, found.sub.id());
    let sub = &*found.sub;
    let id = sub.id();
    if !refs.is_empty() && refs.iter().all(valued) {
        return Some(Target { id, sub: sub.clone(), how: How::Value, negated: false, whole: one_row(&sub.plan) });
    }
    let names = node.input().names();
    match refs.as_slice() {
        [Reference::Exists] => Some(Target { id, sub: sub.clone(), how: How::Exists, negated: false, whole: false }),
        [Reference::Quantified(xs, op, all, true)] => {
            let k = sub.outer.len();
            let slots: Vec<Expr> = xs.iter().enumerate().map(|(j, x)| Expr::Outer(k.saturating_add(j), render_expr(x, &names))).collect();
            let ys = (0..xs.len()).map(Expr::Column).collect();
            let holds = compared(slots, *op, ys)?;
            let cond = if *all { Cond::Is(Box::new(holds), Truth::True, true) } else { holds };
            let plan = LogicalPlan::Filter { cond, input: Box::new(sub.plan.clone()) };
            let outer = sub.outer.iter().chain(xs).cloned().collect();
            Some(Target { id, sub: Subquery::new(plan, outer), how: How::Exists, negated: *all, whole: true })
        }
        [Reference::Quantified(xs, op, all, false)] => {
            compared(xs.clone(), *op, xs.clone())?;
            let k = sub.outer.len();
            let outer = sub.outer.iter().chain(xs).cloned().collect();
            let how = How::Quantified { op: *op, all: *all, slots: (k..k.saturating_add(xs.len())).collect() };
            Some(Target { id, sub: Subquery::new(sub.plan.clone(), outer), how, negated: false, whole: true })
        }
        _ => None,
    }
}

/// Whether `r` reads a value of the subquery's one row, which of a
/// comparison is one `compared` can write.
fn valued(r: &Reference) -> bool {
    match r {
        Reference::Value(None) => true,
        Reference::Value(Some((xs, op))) => compared(xs.clone(), *op, xs.clone()).is_some(),
        Reference::Exists | Reference::Quantified(..) | Reference::Other => false,
    }
}

/// [X3.135-1986 §5.11 GR2; ISO/IEC 9075:1992 §8.2 GR7] The comparison of
/// the row `xs` with the row `ys` by `op`. Of one value it is that
/// comparison; of more, = the AND of each pair's and <> the OR, whose
/// truth tables GR7 gives the row comparison; none of other operators.
pub(super) fn compared(xs: Vec<Expr>, op: CompOp, ys: Vec<Expr>) -> Option<Cond> {
    if xs.len() != ys.len() {
        return None;
    }
    if let ([x], [y]) = (xs.as_slice(), ys.as_slice()) {
        return Some(Cond::Compare(x.clone(), op, y.clone()));
    }
    let pairs = xs.into_iter().zip(ys).map(|(x, y)| Cond::Compare(x, op, y)).collect();
    match op {
        CompOp::Eq => and_all(pairs),
        CompOp::Ne => or_all(pairs),
        CompOp::Lt | CompOp::Gt | CompOp::Le | CompOp::Ge => None,
    }
}
