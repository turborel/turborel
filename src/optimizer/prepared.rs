//! Equivalent scalar implementations prepared from constant operands.
//! Constant folding also discovers operands constructed from parameters.
//! Failing or warning expressions remain for runtime, preserving skipped
//! CASE branches (D15). Parser limits bound recursion.

use std::sync::Arc;

use crate::db::State;
use crate::logical::{Cond, Expr};
use crate::memory::MemoryBudget;
use crate::value::Value;

pub(crate) fn budget(state: &State) -> Option<&Arc<MemoryBudget>> {
    state.optimizer.enabled(super::CONSTANT_FOLDING, &state.settings).then_some(&state.memory)
}

pub(crate) fn expression(expr: Expr, budget: Option<&Arc<MemoryBudget>>) -> Expr {
    let Some(budget) = budget else { return expr };
    super::folding::fold_expr(&expr, budget).unwrap_or(expr)
}

pub(crate) fn condition(cond: Cond, budget: Option<&Arc<MemoryBudget>>) -> Cond {
    match budget {
        Some(budget) => cond.map_exprs(&mut |expr| expression(expr, Some(budget))),
        None => cond,
    }
}

/// The outer columns are constant for one index probe. Only prepare
/// scalar implementations: expressions and the complete condition stay
/// in place, so an empty probe evaluates nothing and errors stay lazy.
/// Parser limits bound this traversal as they bound ordinary preparation.
pub(crate) fn joining(cond: &Cond, outer: &[Value], budget: &Arc<MemoryBudget>) -> Cond {
    cond.clone().map_exprs(&mut |expr| joined(expr, outer, budget))
}

fn joined(expr: Expr, outer: &[Value], budget: &Arc<MemoryBudget>) -> Expr {
    let expr = expr.map_children(&mut |child| joined(child, outer, budget));
    let Expr::Call(function, args) = &expr else { return expr };
    let constants: Vec<_> = args
        .iter()
        .map(|arg| match arg {
            Expr::Column(at) => outer.get(*at),
            Expr::Const(value) => Some(value),
            Expr::Arith(..)
            | Expr::Neg(_)
            | Expr::Call(..)
            | Expr::Outer(..)
            | Expr::Dynamic(_)
            | Expr::Case(..)
            | Expr::Cast(..)
            | Expr::Domain(..)
            | Expr::Warned(..)
            | Expr::SortKey(..)
            | Expr::Truth(_)
            | Expr::NextValue(_)
            | Expr::Invoked(_)
            | Expr::Subquery(..) => None,
        })
        .collect();
    match function.0.specialize(&constants, budget) {
        Some(prepared) => Expr::Call(prepared, args.clone()),
        None => expr,
    }
}

pub(super) fn call(expr: &Expr, budget: &Arc<MemoryBudget>) -> Option<Expr> {
    let Expr::Call(function, args) = expr else { return None };
    let constants: Vec<_> = args.iter().map(|arg| if let Expr::Const(value) = arg { Some(value) } else { None }).collect();
    function.0.specialize(&constants, budget).map(|prepared| Expr::Call(prepared, args.clone()))
}
