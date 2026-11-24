//! \[DESIGN\] Subqueries of existence and quantified comparisons made mark
//! joins (RULES.md D641), after Neumann, Leis and Kemper, "The Complete
//! Story of Joins (in HyPer)" (BTW 2017), §3.3. EXISTS of a correlated
//! subquery, or a quantified comparison with one, that is not a conjunct
//! of WHERE or HAVING, so no semi join keeps the rows it decides (`semi.rs`),
//! stands under OR or NOT, in a CASE or in a select list. There its value
//! is true, false or unknown for each row [X3.135-1986 §5.16 GR2, §5.17
//! GR2], which is the mark of the row in the mark join of the node's rows
//! with the subquery's on the condition that correlates them
//! (`logical/mark.rs`); the place reads the mark.
//!
//! `x op SOME s` marks by `x op y`, `x op ALL s` is NOT of the mark by
//! `NOT (x op y)`, written as the comparison of the negated operator, so
//! that `x <> ALL s`, NOT IN, marks by `x = y` and is hashed.
//!
//! The rows matched are the subquery's, as `semi.rs` makes them: directly,
//! if under its projections, a DISTINCT and its filters it reads no outer
//! reference, else of each binding the subquery has rows for, joined on
//! the binding's keys. The join reads its right rows only until one makes
//! the mark true, and judges its conditions on the pairs its keys keep, so
//! it is made only of a subquery and comparison that cannot raise (D15).

use super::found::{self, Found, Node, Reference};
use super::joined::Replacing;
use super::{Bound, How, Target, compared, domain, pure, semi, simple, typed};
use crate::logical::{Cond, Expr, LogicalPlan, MarkJoin, Subquery, and_all};
use crate::optimizer::{Rule, RuleContext};
use crate::value::CompOp;

/// The name of the rule.
pub const MARK_JOINS: &str = "mark_joins";

/// The rule that makes subqueries of existence and quantified comparisons
/// that no semi join takes mark joins (see the module's documentation).
pub struct MarkJoins;

impl Rule for MarkJoins {
    fn name(&self) -> &'static str {
        MARK_JOINS
    }
    fn apply(&self, plan: &LogicalPlan, ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        let node = Node::of(plan)?;
        found::candidates(&node).iter().find_map(|f| marked(&node, f, ctx))
    }
}

/// The values a quantified comparison compares, its operator, and whether
/// ALL; none of EXISTS.
type Quantified = Option<(Vec<Expr>, CompOp, bool)>;

/// The one use `node` makes of `found`'s subquery, if it is EXISTS or a
/// quantified comparison that is not a conjunct of WHERE or HAVING.
fn used(node: &Node<'_>, found: &Found) -> Option<Quantified> {
    let refs = found::references(node, found.sub.id());
    match refs.as_slice() {
        [Reference::Exists] => Some(None),
        [Reference::Quantified(xs, op, all, false)] => Some(Some((xs.clone(), *op, *all))),
        _ => None,
    }
}

/// The node `node`, its subquery `found` made a mark join, if it can be.
fn marked(node: &Node<'_>, found: &Found, ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
    let sub = &*found.sub;
    if sub.outer.is_empty() {
        // The executor reads an uncorrelated subquery once (`exec/subquery.rs`).
        return None;
    }
    let quantified = used(node, found)?;
    let input = node.input();
    let catalog = &*ctx.db.catalog;
    let xs = quantified.as_ref().map_or(&[][..], |(xs, ..)| xs.as_slice());
    let plain = sub.outer.iter().chain(xs).all(|o| typed::plain_key(o, input, catalog));
    // The values compared are computed for every row of the node, as they
    // are where the comparison always stands, but not in a CASE that may
    // leave it unevaluated.
    let xs_raise = found.conditional && xs.iter().any(Expr::can_raise);
    let raises = xs_raise || sub.outer.iter().any(Expr::can_raise) || pure::plan_raises(&sub.plan, ctx);
    let pure = sub.outer.iter().chain(xs).all(super::expr_pure) && pure::plan_pure(&sub.plan);
    if !plain || raises || !pure {
        return None;
    }
    let mut left = Widened::of(input);
    // The values compared are placed first, as the right's columns follow
    // the left's.
    let at: Vec<Expr> = xs.iter().map(|x| Expr::Column(left.column(x))).collect();
    let (cond, right, ys) = matched(sub, &mut left, ctx)?;
    let compared = match &quantified {
        Some((_, op, all)) => {
            let op = if *all { negated(*op) } else { *op };
            let c = compared(at, op, ys)?;
            if c.can_raise() {
                return None;
            }
            Some(c)
        }
        None => None,
    };
    let base = left.width();
    let join = LogicalPlan::MarkJoin(Box::new(MarkJoin { cond, compared, left: left.plan(), right }));
    let all = quantified.as_ref().is_some_and(|(.., all)| *all);
    let target = Target { id: sub.id(), sub: sub.clone(), how: How::Exists, negated: all, whole: true };
    let with = Replacing::marked_at(&target, base);
    let w = input.width();
    Some(match node {
        Node::Filter(c, _) => {
            let filter = LogicalPlan::Filter { cond: with.cond((*c).clone()), input: Box::new(join) };
            LogicalPlan::Project { exprs: (0..w).map(Expr::Column).collect(), input: Box::new(filter) }
        }
        Node::Project(exprs, _) => {
            LogicalPlan::Project { exprs: exprs.iter().map(|e| with.expr(e.clone())).collect(), input: Box::new(join) }
        }
    })
}

/// [ISO/IEC 9075:1992 §8.2 GR7] The operator whose comparison is NOT of
/// `op`'s, of a row as of a value: `<>` is NOT of `=` of rows too.
fn negated(op: CompOp) -> CompOp {
    match op {
        CompOp::Eq => CompOp::Ne,
        CompOp::Ne => CompOp::Eq,
        CompOp::Lt => CompOp::Ge,
        CompOp::Ge => CompOp::Lt,
        CompOp::Gt => CompOp::Le,
        CompOp::Le => CompOp::Gt,
    }
}

/// The node's rows, with the columns a mark join reads of them that are
/// not their columns put after those.
struct Widened<'a> {
    rows: &'a LogicalPlan,
    extra: Vec<Expr>,
}

impl<'a> Widened<'a> {
    fn of(rows: &'a LogicalPlan) -> Widened<'a> {
        Widened { rows, extra: Vec::new() }
    }

    /// The column that holds `e`, an expression over the rows.
    fn column(&mut self, e: &Expr) -> usize {
        let w = self.rows.width();
        if let Expr::Column(c) = e
            && *c < w
        {
            return *c;
        }
        self.extra.push(e.clone());
        w.saturating_add(self.extra.len()).saturating_sub(1)
    }

    /// The width of its rows with the columns put after.
    fn width(&self) -> usize {
        self.rows.width().saturating_add(self.extra.len())
    }

    /// Its rows, projected with the columns put after, if any is.
    fn plan(self) -> LogicalPlan {
        if self.extra.is_empty() {
            return self.rows.clone();
        }
        let mut exprs: Vec<Expr> = (0..self.rows.width()).map(Expr::Column).collect();
        exprs.extend(self.extra);
        LogicalPlan::Project { exprs, input: Box::new(self.rows.clone()) }
    }
}

/// The condition of the mark join of `left` with the rows of `sub`, the
/// plan of those, and the subquery's columns over the pair's: directly
/// (`semi::direct`), else of the rows of each binding, on its keys.
fn matched(sub: &Subquery, left: &mut Widened<'_>, ctx: &RuleContext<'_>) -> Option<(Cond, LogicalPlan, Vec<Expr>)> {
    let n = sub.plan.width();
    if let Some((cond, body, map)) = semi::direct(sub, left.width()) {
        let w = left.width();
        let Some(map) = map else { return Some((cond, body, (0..n).map(|j| Expr::Column(w.saturating_add(j))).collect())) };
        let bw = body.width();
        let mut exprs: Vec<Expr> = (0..bw).map(Expr::Column).collect();
        exprs.extend(map);
        let right = LogicalPlan::Project { exprs, input: Box::new(body) };
        return Some((cond, right, (0..n).map(|j| Expr::Column(w.saturating_add(bw).saturating_add(j))).collect()));
    }
    let bound = simple::pulled(sub, ctx).filter(|b| b.defaults.is_none());
    let bound = bound.or_else(|| domain::pushed(sub, left.rows))?;
    keyed(bound, left, n)
}

/// Of `bound`, the rows of each binding, its keys first, then the
/// subquery's `n` columns: the condition on its keys of the mark join of
/// `left` with them, the plan, and the subquery's columns over the pair's.
fn keyed(bound: Bound, left: &mut Widened<'_>, n: usize) -> Option<(Cond, LogicalPlan, Vec<Expr>)> {
    let at: Vec<usize> = bound.keys.iter().map(|e| left.column(e)).collect();
    let w = left.width();
    let k = at.len();
    let nd = bound.not_distinct;
    let cond = and_all(at.iter().enumerate().map(|(i, &a)| key_matched(a, w.saturating_add(i), nd)).collect())?;
    let ys = (0..n).map(|j| Expr::Column(w.saturating_add(k).saturating_add(j))).collect();
    Some((cond, bound.plan, ys))
}

/// That column `a` matches column `b`, as IS NOT DISTINCT FROM if
/// `not_distinct`, else as =.
fn key_matched(a: usize, b: usize, not_distinct: bool) -> Cond {
    let (a, b) = (Expr::Column(a), Expr::Column(b));
    if not_distinct { Cond::Not(Box::new(Cond::Distinct(a, b))) } else { Cond::Compare(a, CompOp::Eq, b) }
}
