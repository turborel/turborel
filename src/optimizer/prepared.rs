//! Equivalent scalar implementations prepared from constant operands.
//! Constant folding also discovers operands constructed from parameters.
//! Failing or warning expressions remain for runtime, preserving skipped
//! CASE branches (D15). Parser limits bound recursion.

use std::sync::Arc;

use crate::db::State;
use crate::logical::{Cond, Expr};
use crate::memory::MemoryBudget;

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

pub(super) fn call(expr: &Expr, budget: &Arc<MemoryBudget>) -> Option<Expr> {
    let Expr::Call(function, args) = expr else { return None };
    let constants: Vec<_> = args.iter().map(|arg| if let Expr::Const(value) = arg { Some(value) } else { None }).collect();
    function.0.specialize(&constants, budget).map(|prepared| Expr::Call(prepared, args.clone()))
}
