//! \[DESIGN\] (RULES.md D85) A comparison of a call with a constant decided
//! by the call's function without the call's value, where the function
//! does so (`ScalarFunction::call_compared`).

use super::{Subqueries, argument, compared_read, known, operand};
use crate::error::Result;
use crate::logical::{Expr, Truth};
use crate::storage::Row;
use crate::value::CompOp;

/// The truth of "`a op b`" of `row` [X3.135-1986 §5.11 GR2, GR3]: decided
/// by the call's function where it does (`call_compared`), else of the
/// values compared.
pub(super) fn compared((a, op, b): (&Expr, CompOp, &Expr), row: &Row, subqueries: &dyn Subqueries) -> Result<Truth> {
    match call_compared((a, op, b), row, subqueries)? {
        Some(decided) => Ok(decided),
        // Each operand read where it is held, if it is (`operand`).
        None => compared_read(&*operand(a, row, subqueries)?, op, &*operand(b, row, subqueries)?),
    }
}

/// "`f(x, y) op value`", or the call second, decided by its function
/// without the call's value, where it does (`ScalarFunction::call_compared`),
/// of arguments the row, constants or the statement's slots give; none to make the call and
/// compare its value.
fn call_compared((a, op, b): (&Expr, CompOp, &Expr), row: &Row, subqueries: &dyn Subqueries) -> Result<Option<Truth>> {
    let (function, args, op, value) = if let (Expr::Call(function, args), Expr::Const(value)) = (a, b) {
        (function, args, op, value)
    } else if let (Expr::Const(value), Expr::Call(function, args)) = (a, b) {
        (function, args, op.flip(), value)
    } else {
        return Ok(None);
    };
    let [left, right] = args.as_slice() else { return Ok(None) };
    let (Some(left), Some(right)) = (argument(left, row, subqueries), argument(right, row, subqueries)) else { return Ok(None) };
    let decided = function.0.call_compared(&[left?, right?], op, value);
    Ok(decided.transpose()?.map(known))
}
