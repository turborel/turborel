//! Combine adjacent column selections, including the projection that
//! restores a reversed join's output order. The executor can then select
//! the final values directly from the pair (D31). Computed expressions
//! remain separate so an unused output cannot hide an error (D15).

use super::Plan;
use crate::logical::Expr;

pub(super) fn collapsed(mut exprs: Vec<Expr>, mut input: Plan) -> (Vec<Expr>, Plan) {
    while let Plan::Project { exprs: through, .. } = &input {
        let Some(mapped) = columns(&exprs, through) else { break };
        let Plan::Project { input: inner, .. } = input else { break };
        exprs = mapped;
        input = *inner;
    }
    (exprs, input)
}

fn columns(exprs: &[Expr], through: &[Expr]) -> Option<Vec<Expr>> {
    if through.iter().any(|expr| !matches!(expr, Expr::Column(_))) {
        return None;
    }
    exprs
        .iter()
        .map(|expr| {
            let Expr::Column(column) = expr else { return None };
            through.get(*column).cloned()
        })
        .collect()
}

#[cfg(test)]
mod tests;
