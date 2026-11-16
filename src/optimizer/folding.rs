//! Constant evaluation and optional scalar preparation preserve lazy
//! runtime errors and warnings (D15, D53). Prepared state draws on the
//! same database memory budget as its other retained caches (D38).

use std::sync::Arc;

use super::{CONSTANT_FOLDING, Rule, RuleContext};
use crate::error::Warnings;
use crate::exec::{Heard, NoSubqueries, eval, eval_cond};
use crate::logical::{Cond, Expr, LogicalPlan, Truth};
use crate::memory::MemoryBudget;

/// Evaluate column-free subexpressions once, at planning time. A subexpression
/// whose evaluation fails ("1/0"), or raises a warning (a CAST that cuts a
/// string [ISO/IEC 9075:1992 §6.10 GR5c ii]), is left for run time, so the
/// condition is still raised exactly when the standard requires. Calls are
/// folded only for functions that declare themselves deterministic.
pub struct ConstantFolding;

/// The folded form of `e`, or `None` if nothing in it folds: its parts
/// folded, and it computed if they are all constants.
pub(super) fn fold_expr(e: &Expr, budget: &Arc<MemoryBudget>) -> Option<Expr> {
    let mut changed = false;
    let folded = e.clone().map_children(&mut |child| match fold_expr(&child, budget) {
        Some(done) => {
            changed = true;
            done
        }
        None => child,
    });
    if computable(&folded) {
        // A failing subexpression is left for run time, which raises the error
        // when the standard requires it [X3.135-1986 §3.3]; one that warns
        // too, for the statement that runs it to complete with the warning
        // [ISO/IEC 9075:1992 §3.3.4.1].
        let warnings = Warnings::default();
        if let (Ok(v), None) = (eval(&folded, &Vec::new(), &Heard(&warnings)), warnings.take()) {
            return Some(Expr::Const(v));
        }
    }
    if let Some(prepared) = super::prepared::call(&folded, budget) {
        return Some(prepared);
    }
    changed.then_some(folded)
}

/// Whether `e`, not a constant, can be computed now: an operation on
/// constants alone, calling only a deterministic function, its condition,
/// if a truth value's, of no subquery; and no NEXT VALUE FOR, which draws
/// a value each row [ISO/IEC 9075-2:2023 §6.14 GR1].
fn computable(e: &Expr) -> bool {
    let mut constant =
        !matches!(e, Expr::Column(_) | Expr::Const(_) | Expr::Outer(..) | Expr::Dynamic(_) | Expr::Subquery(..) | Expr::NextValue(_));
    if let Expr::Call(func, _) = e {
        constant = constant && func.0.deterministic();
    }
    if let Expr::Truth(c) = e {
        constant = constant && c.subqueries().is_empty();
    }
    e.for_each_child(&mut |child| constant = constant && matches!(child, Expr::Const(_)));
    constant
}

/// The folded form of `c`, or `None` if nothing in it folds.
fn fold_cond(c: &Cond, budget: &Arc<MemoryBudget>) -> Option<Cond> {
    match c {
        // [ISO/IEC 9075-2:2023 §4.6.1] a constant's truth.
        Cond::Value(Expr::Const(v)) => Some(Cond::Const(Truth::of(v))),
        Cond::Value(e) => {
            fold_expr(e, budget).map(|folded| if let Expr::Const(v) = &folded { Cond::Const(Truth::of(v)) } else { Cond::Value(folded) })
        }
        Cond::Const(_)
        | Cond::IsNull(..)
        | Cond::Like(..)
        | Cond::Exists(_)
        | Cond::Quantified(..)
        | Cond::Is(..)
        | Cond::Unique(..)
        | Cond::Match { .. }
        | Cond::In(..) => None,
        Cond::Or(a, b) | Cond::And(a, b) => match (fold_cond(a, budget), fold_cond(b, budget)) {
            (None, None) => None,
            (left, right) => {
                let left = Box::new(left.unwrap_or_else(|| (**a).clone()));
                let right = Box::new(right.unwrap_or_else(|| (**b).clone()));
                Some(if matches!(c, Cond::Or(..)) { Cond::Or(left, right) } else { Cond::And(left, right) })
            }
        },
        Cond::Not(a) => fold_cond(a, budget).map(|a| Cond::Not(Box::new(a))),
        Cond::Compare(a, op, b) => fold_pair((a, b), |a, b| Cond::Compare(a, *op, b), budget),
        Cond::Distinct(a, b) => fold_pair((a, b), Cond::Distinct, budget),
    }
}

/// The folded form of the condition `of` makes of `a` and `b`, a
/// comparison or a `<distinct predicate>`: its truth if both fold to
/// constants, else of what folds; `None` if nothing does.
fn fold_pair((a, b): (&Expr, &Expr), of: impl Fn(Expr, Expr) -> Cond, budget: &Arc<MemoryBudget>) -> Option<Cond> {
    let (fa, fb) = (fold_expr(a, budget), fold_expr(b, budget));
    let (a2, b2) = (fa.clone().unwrap_or_else(|| a.clone()), fb.clone().unwrap_or_else(|| b.clone()));
    if let (Expr::Const(_), Expr::Const(_)) = (&a2, &b2) {
        let changed = fa.is_some() || fb.is_some();
        let cmp = of(a2, b2);
        return match eval_cond(&cmp, &Vec::new(), &NoSubqueries) {
            Ok(t) => Some(Cond::Const(t)),
            // Left for run time, as in `fold_expr`.
            Err(_) => changed.then_some(cmp),
        };
    }
    if fa.is_none() && fb.is_none() { None } else { Some(of(a2, b2)) }
}

impl Rule for ConstantFolding {
    fn name(&self) -> &'static str {
        CONSTANT_FOLDING
    }
    fn apply(&self, plan: &LogicalPlan, ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        let budget = &ctx.db.memory;
        match plan {
            LogicalPlan::Filter { cond, input } => fold_cond(cond, budget).map(|cond| LogicalPlan::Filter { cond, input: input.clone() }),
            LogicalPlan::OuterJoin { kind, cond, left, right } => {
                fold_cond(cond, budget).map(|cond| LogicalPlan::OuterJoin { kind: *kind, cond, left: left.clone(), right: right.clone() })
            }
            LogicalPlan::Project { exprs, input } => {
                let folded: Vec<Option<Expr>> = exprs.iter().map(|e| fold_expr(e, budget)).collect();
                if folded.iter().all(Option::is_none) {
                    return None;
                }
                let exprs = folded.into_iter().zip(exprs).map(|(f, e)| f.unwrap_or_else(|| e.clone())).collect();
                Some(LogicalPlan::Project { exprs, input: input.clone() })
            }
            LogicalPlan::Scan { .. }
            | LogicalPlan::Product { .. }
            | LogicalPlan::Aggregate { .. }
            | LogicalPlan::Distinct { .. }
            | LogicalPlan::SetOp { .. }
            | LogicalPlan::Values { .. }
            | LogicalPlan::System { .. }
            | LogicalPlan::Sort { .. }
            | LogicalPlan::Limit { .. }
            | LogicalPlan::Recursive { .. }
            | LogicalPlan::Unnest(..)
            | LogicalPlan::Window(..)
            | LogicalPlan::Recognize(..)
            | LogicalPlan::Polymorphic(..)
            | LogicalPlan::WorkTable { .. }
            | LogicalPlan::Transition { .. } => None,
        }
    }
}
