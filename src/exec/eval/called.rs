//! Calls of scalar functions in an expression evaluated for a row: their
//! arguments evaluated, or borrowed from the row where they are its own.

use super::{Subqueries, column, context, eval, values, warn};
use crate::error::Result;
use crate::functions::ScalarRef;
use crate::logical::Expr;
use crate::storage::Row;
use crate::value::Value;

/// A call of a function that may raise a warning (`ScalarFunction::warns`)
/// [ISO/IEC 9075:1992 §3.3.4.1]: its result, the warning raised, if any,
/// as a CAST's is.
pub(super) fn called_warned(function: &ScalarRef, args: &[Expr], row: &Row, subqueries: &dyn Subqueries) -> Result<Value> {
    let values = args.iter().map(|arg| eval(arg, row, subqueries)).collect::<Result<Vec<_>>>()?;
    let (value, warning) = function.0.call_warned(&values, subqueries.temp_lobs())?;
    if let Some(state) = warning {
        warn(subqueries, state);
    }
    Ok(value)
}

/// The usual unary and binary calls need no heap allocation for their
/// argument list. Arrays preserve left-to-right evaluation and drop earlier
/// values if a later argument fails, just as collecting into a vector does.
pub(super) fn called(function: &ScalarRef, args: &[Expr], row: &Row, subqueries: &dyn Subqueries) -> Result<Value> {
    if function.0.in_statement() || function.0.uses_context(args.len()) {
        return context::reading(function, &values(args, row, subqueries)?, subqueries);
    }
    let temp = subqueries.temp_lobs();
    match args {
        [] => function.0.call_with(&[], temp),
        [one] => match argument(one, row) {
            Some(value) => function.0.call_borrowed(&[value?], temp),
            None => function.0.call_with(&[eval(one, row, subqueries)?], temp),
        },
        [left, right] => match (argument(left, row), argument(right, row)) {
            (Some(left), Some(right)) => function.0.call_borrowed(&[left?, right?], temp),
            _ => function.0.call_with(&[eval(left, row, subqueries)?, eval(right, row, subqueries)?], temp),
        },
        _ => {
            let values = args.iter().map(|arg| eval(arg, row, subqueries)).collect::<Result<Vec<_>>>()?;
            function.0.call_with(&values, temp)
        }
    }
}

/// Direct arguments already live long enough for a call. In particular,
/// borrowing an extension value avoids atomic reference-count traffic on
/// its type and payload shared by concurrent readers.
pub(super) fn argument<'a>(expr: &'a Expr, row: &'a Row) -> Option<Result<&'a Value>> {
    match expr {
        Expr::Column(at) => Some(column(row, *at)),
        Expr::Const(value) => Some(Ok(value)),
        Expr::Neg(_)
        | Expr::Arith(..)
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
    }
}
