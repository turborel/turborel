//! \[DESIGN\] Subqueries of existence made semi joins and antijoins
//! (RULES.md D541). A conjunct of WHERE or HAVING that is EXISTS of a
//! correlated subquery, NOT EXISTS of one, or a quantified comparison made
//! EXISTS or NOT EXISTS (`unnesting.rs`) keeps a row of the node's input
//! if a row of the subquery is there for its binding, or if none is
//! [X3.135-1986 §5.17 GR2, GR3]. That is the semi join, or the antijoin,
//! of the input with the subquery's rows on the condition that correlates
//! them, after Neumann and Kemper, "Unnesting Arbitrary Queries" (2015),
//! §2.
//!
//! 1. Directly, if under its projections, a DISTINCT, which existence does
//!    not see, and its filters the subquery reads no outer reference: the
//!    conjuncts of those filters that read one are the join's condition,
//!    each outer reference the value it is bound to, and the others stay
//!    on the right.
//! 2. Else of the rows of each binding the subquery has, as the other
//!    unnesting makes them (`simple.rs`, `domain.rs`), joined on the
//!    binding's keys.
//!
//! The join stops at the first row that matches, so it is made only of a
//! subquery and a condition that cannot raise (D15). The node's other
//! conjuncts filter its input below the join, so they are evaluated on
//! every row, as they were.
//!
//! A NOT IN, `NOT (x = SOME s)`, is the antijoin on `(x = y) IS NOT
//! FALSE`, which a null on either side satisfies, as the standard's truth
//! tables ask [ISO/IEC 9075:1992 §8.7 GR2]; `x <> ALL s` the antijoin on
//! `(x <> y) IS NOT TRUE`.

use super::found::{self, Found, Node};
use super::{Bound, How, Target, compared, domain, pure, scalar, simple, target, typed};
use crate::logical::{
    Cond, Expr, LogicalPlan, Quantifier, Subquery, Truth, and_all, cond_has_outer, plan_has_outer, render_expr, split_and,
};
use crate::optimizer::{Rule, RuleContext};
use crate::value::CompOp;

/// The name of the rule.
pub const SEMI_JOINS: &str = "semi_joins";

/// The rule that makes subqueries of existence semi joins and antijoins
/// (see the module's documentation).
pub struct SemiJoins;

impl Rule for SemiJoins {
    fn name(&self) -> &'static str {
        SEMI_JOINS
    }
    fn apply(&self, plan: &LogicalPlan, ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        let LogicalPlan::Filter { cond, .. } = plan else { return None };
        let node = Node::of(plan)?;
        // A comparison with a subquery of one row at most (`scalar.rs`).
        found::candidates(&node).iter().find_map(|f| semi_joined(&node, cond, f, ctx)).or_else(|| scalar::joined(cond, node.input(), ctx))
    }
}

/// The node `node`, of condition `cond`, its subquery `found` made a semi
/// join or an antijoin, if it can be.
fn semi_joined(node: &Node<'_>, cond: &Cond, found: &Found, ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
    let (target, anti, rest) = if let Some(made) = negated(node, cond, found) {
        made
    } else {
        let target = target(node, found)?;
        let (anti, rest) = placed(cond, &target)?;
        (target, anti, rest)
    };
    let sub = &target.sub;
    let input = node.input();
    let catalog = &*ctx.db.catalog;
    // Every row of the subquery may go unread, and its bindings are hashed.
    let plain = sub.outer.iter().all(|o| typed::plain_key(o, input, catalog));
    let raises = sub.outer.iter().any(Expr::can_raise) || pure::plan_raises(&sub.plan, ctx);
    let pure = sub.outer.iter().all(super::expr_pure) && pure::plan_pure(&sub.plan);
    if !plain || raises || !pure {
        return None;
    }
    let left = match and_all(rest) {
        Some(c) => LogicalPlan::Filter { cond: c, input: Box::new(input.clone()) },
        None => input.clone(),
    };
    let w = input.width();
    if let Some((on, right, _)) = direct(sub, w) {
        return Some(LogicalPlan::SemiJoin { anti, cond: on, left: Box::new(left), right: Box::new(right) });
    }
    let bound = simple::pulled(sub, ctx).filter(|b| b.defaults.is_none());
    let bound = bound.or_else(|| domain::pushed(sub, input))?;
    keyed((anti, bound), left, w)
}

/// Whether the conjunct of `cond` that uses `target`'s subquery makes an
/// antijoin, and the other conjuncts; none unless one conjunct is EXISTS
/// of it, NOT EXISTS, or a quantified comparison with it.
fn placed(cond: &Cond, target: &Target) -> Option<(bool, Vec<Cond>)> {
    if !matches!(target.how, How::Exists) {
        return None;
    }
    let mut conjuncts = Vec::new();
    split_and(cond.clone(), &mut conjuncts);
    let (mut anti, mut rest) = (None, Vec::new());
    for c in conjuncts {
        match (use_of(&c, target), anti) {
            (Some(negated), None) => anti = Some(negated),
            (Some(_), Some(_)) => return None,
            (None, Some(_) | None) => rest.push(c),
        }
    }
    Some((anti?, rest))
}

/// Of a conjunct of `cond` that is NOT of a quantified comparison with
/// `found`'s subquery, and its one use, the subquery made EXISTS of the
/// rows that decide it, whether an antijoin, and the other conjuncts.
/// `NOT (x op SOME s)` is true if no row's `x op y` is true or unknown, the
/// antijoin on `(x op y) IS NOT FALSE`; `NOT (x op ALL s)` if some row's is
/// false, the semi join on `(x op y) IS FALSE` [X3.135-1986 §5.16 GR2].
fn negated(node: &Node<'_>, cond: &Cond, found: &Found) -> Option<(Target, bool, Vec<Cond>)> {
    let sub = &*found.sub;
    if found::references(node, sub.id()).len() != 1 {
        return None;
    }
    let mut conjuncts = Vec::new();
    split_and(cond.clone(), &mut conjuncts);
    let at = conjuncts.iter().position(|c| quantified_of(c, sub.id()).is_some())?;
    let c = conjuncts.remove(at);
    let (xs, op, all) = quantified_of(&c, sub.id())?;
    let k = sub.outer.len();
    let names = node.input().names();
    let slots = xs.iter().enumerate().map(|(j, x)| Expr::Outer(k.saturating_add(j), render_expr(x, &names))).collect();
    let holds = compared(slots, op, (0..xs.len()).map(Expr::Column).collect())?;
    let decides = Cond::Is(Box::new(holds), Truth::False, !all);
    let plan = LogicalPlan::Filter { cond: decides, input: Box::new(sub.plan.clone()) };
    let outer = sub.outer.iter().chain(xs).cloned().collect();
    let target = Target { id: sub.id(), sub: Subquery::new(plan, outer), how: How::Exists, negated: !all, whole: true };
    Some((target, !all, conjuncts))
}

/// Of `c`, if it is NOT of a quantified comparison with the subquery `id`,
/// the values compared, the operator, and whether ALL.
fn quantified_of(c: &Cond, id: u64) -> Option<(&[Expr], CompOp, bool)> {
    let Cond::Not(inner) = c else { return None };
    let Cond::Quantified(xs, op, q, s) = &**inner else { return None };
    let all = match q {
        Quantifier::All => true,
        Quantifier::Some => false,
        Quantifier::One => return None,
    };
    (s.id() == id).then_some((xs.as_slice(), *op, all))
}

/// Of the conjunct `c`, if it is EXISTS of `target`'s subquery or NOT
/// EXISTS, or the quantified comparison it was made of, whether NOT.
fn use_of(c: &Cond, target: &Target) -> Option<bool> {
    match c {
        Cond::Exists(s) => (s.id() == target.id).then_some(false),
        Cond::Not(inner) => match &**inner {
            Cond::Exists(s) => (s.id() == target.id).then_some(true),
            Cond::Const(_)
            | Cond::Compare(..)
            | Cond::Distinct(..)
            | Cond::IsNull(..)
            | Cond::Like(..)
            | Cond::In(..)
            | Cond::Value(_)
            | Cond::And(..)
            | Cond::Or(..)
            | Cond::Not(_)
            | Cond::Is(..)
            | Cond::Unique(..)
            | Cond::Quantified(..)
            | Cond::Match { .. } => None,
        },
        Cond::Quantified(.., s) => (s.id() == target.id).then_some(target.negated),
        Cond::Const(_)
        | Cond::Compare(..)
        | Cond::Distinct(..)
        | Cond::IsNull(..)
        | Cond::Like(..)
        | Cond::In(..)
        | Cond::Value(_)
        | Cond::And(..)
        | Cond::Or(..)
        | Cond::Is(..)
        | Cond::Unique(..)
        | Cond::Match { .. } => None,
    }
}

/// The condition of the semi join of rows `w` columns wide with the
/// subquery `sub`'s rows, the plan of those, and the subquery's columns
/// over that plan's if they are not its columns, if under its projections,
/// a DISTINCT and its filters it reads no outer reference, and the
/// conjuncts of those filters that do read one cannot raise and hold no
/// subquery.
pub(super) fn direct(sub: &Subquery, w: usize) -> Option<(Cond, LogicalPlan, Option<Vec<Expr>>)> {
    let (conjuncts, body, map) = peeled(&sub.plan, 0)?;
    if plan_has_outer(&body) {
        return None;
    }
    let (correlated, own): (Vec<Cond>, Vec<Cond>) = conjuncts.into_iter().partition(cond_has_outer);
    let mut on = Vec::new();
    for c in correlated {
        let lifted = c.map_exprs(&mut |e| lifted(e, &sub.outer, w));
        if lifted.can_raise() || lifted.has_subquery() {
            return None;
        }
        on.push(lifted);
    }
    let cond = and_all(on).unwrap_or(Cond::Const(Truth::True));
    let right = match and_all(own) {
        Some(c) => LogicalPlan::Filter { cond: c, input: Box::new(body) },
        None => body,
    };
    Some((cond, right, map))
}

/// What `peeled` makes of a plan: the conjuncts of its filters and the
/// plan under them, and its columns as expressions over that plan's, or
/// none if they are its columns.
type Peeled = (Vec<Cond>, LogicalPlan, Option<Vec<Expr>>);

/// The conjuncts of `plan`'s filters, over the rows of what is under them,
/// and that, with its projections, which the filters' conditions are
/// written through, and a DISTINCT taken off; `depth` nodes down.
fn peeled(plan: &LogicalPlan, depth: usize) -> Option<Peeled> {
    if depth > crate::logical::MAX_JUDGED_DEPTH {
        return None;
    }
    let deeper = depth.saturating_add(1);
    match plan {
        LogicalPlan::Filter { cond, input } => {
            let (mut conjuncts, body, map) = peeled(input, deeper)?;
            let written = cond.clone().map_exprs(&mut |e| through(e, map.as_deref()));
            split_and(written, &mut conjuncts);
            Some((conjuncts, body, map))
        }
        LogicalPlan::Project { exprs, input } => {
            let (conjuncts, body, map) = peeled(input, deeper)?;
            let exprs = exprs.iter().map(|e| through(e.clone(), map.as_deref())).collect();
            Some((conjuncts, body, Some(exprs)))
        }
        LogicalPlan::Distinct { input } => peeled(input, deeper),
        LogicalPlan::Scan { .. }
        | LogicalPlan::Product { .. }
        | LogicalPlan::Aggregate { .. }
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
        | LogicalPlan::Polymorphic(_) => Some((Vec::new(), plan.clone(), None)),
    }
}

/// `e`, over a projection's columns `map`, over the projection's input.
fn through(e: Expr, map: Option<&[Expr]>) -> Expr {
    if let (Expr::Column(c), Some(map)) = (&e, map) {
        return map.get(*c).cloned().unwrap_or(e);
    }
    e.map_children(&mut |x| through(x, map))
}

/// `e`, over the subquery's rows and its outer references `outer`, over
/// the pair of a row `w` columns wide and a subquery row.
fn lifted(e: Expr, outer: &[Expr], w: usize) -> Expr {
    if let Expr::Column(c) = e {
        return Expr::Column(c.saturating_add(w));
    }
    if let Expr::Outer(slot, _) = &e
        && let Some(bound) = outer.get(*slot)
    {
        return bound.clone();
    }
    e.map_children(&mut |x| lifted(x, outer, w))
}

/// The semi join, or of `anti` the antijoin, of `left`, rows `w` columns
/// wide, with the rows of each binding `bound` holds, on its keys: of
/// keys that are columns, directly, else of `left` with its keys, then
/// projected to its columns.
fn keyed((anti, bound): (bool, Bound), left: LogicalPlan, w: usize) -> Option<LogicalPlan> {
    let k = bound.keys.len();
    let right = Box::new(LogicalPlan::Project { exprs: (0..k).map(Expr::Column).collect(), input: Box::new(bound.plan) });
    let nd = bound.not_distinct;
    if bound.keys.iter().all(|e| matches!(e, Expr::Column(_))) {
        let cond = and_all(bound.keys.into_iter().enumerate().map(|(i, e)| matched(e, w.saturating_add(i), nd)).collect())?;
        return Some(LogicalPlan::SemiJoin { anti, cond, left: Box::new(left), right });
    }
    let mut with_keys: Vec<Expr> = (0..w).map(Expr::Column).collect();
    with_keys.extend(bound.keys);
    let left = Box::new(LogicalPlan::Project { exprs: with_keys, input: Box::new(left) });
    let wk = w.saturating_add(k);
    let cond = and_all((0..k).map(|i| matched(Expr::Column(w.saturating_add(i)), wk.saturating_add(i), nd)).collect())?;
    let semi = LogicalPlan::SemiJoin { anti, cond, left, right };
    Some(LogicalPlan::Project { exprs: (0..w).map(Expr::Column).collect(), input: Box::new(semi) })
}

/// That `a` matches the column `b`, as IS NOT DISTINCT FROM if
/// `not_distinct`, else as =.
fn matched(a: Expr, b: usize, not_distinct: bool) -> Cond {
    if not_distinct { Cond::Not(Box::new(Cond::Distinct(a, Expr::Column(b)))) } else { Cond::Compare(a, CompOp::Eq, Expr::Column(b)) }
}
