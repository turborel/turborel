//! Calls of scalar functions in an expression evaluated for a row: their
//! arguments evaluated, or borrowed from the row where they are its own.

use std::borrow::Cow;

use super::{Subqueries, column, context, eval, values, warn};
use crate::error::Result;
use crate::functions::ScalarRef;
use crate::logical::Expr;
use crate::storage::Row;
use crate::value::{Value, held};

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
        [one] => match argument(one, row, subqueries) {
            Some(value) => function.0.call_borrowed(&[value?], temp),
            None => function.0.call_with(&[eval(one, row, subqueries)?], temp),
        },
        [left, right] => match (argument(left, row, subqueries), argument(right, row, subqueries)) {
            (Some(left), Some(right)) => function.0.call_borrowed(&[left?, right?], temp),
            _ => function.0.call_with(&[eval(left, row, subqueries)?, eval(right, row, subqueries)?], temp),
        },
        _ => {
            let values = args.iter().map(|arg| eval(arg, row, subqueries)).collect::<Result<Vec<_>>>()?;
            function.0.call_with(&values, temp)
        }
    }
}

/// The value of `expr` for `row`: borrowed where it is held (`argument`),
/// else evaluated. An operand of an operator or a comparison is read so,
/// with no copy of a value the row, a constant or the statement holds.
///
/// # Errors
///
/// As `eval`.
pub(super) fn operand<'a>(expr: &'a Expr, row: &'a Row, subqueries: &'a dyn Subqueries) -> Result<Cow<'a, Value>> {
    match argument(expr, row, subqueries) {
        Some(held) => held.map(Cow::Borrowed),
        None => eval(expr, row, subqueries).map(Cow::Owned),
    }
}

/// Direct arguments already live long enough for a call. In particular,
/// borrowing an extension value avoids atomic reference-count traffic on
/// its type and payload shared by concurrent readers. An outer reference
/// is borrowed where the statement holds it, and so is a slot whose value
/// is one of its type already, which its cast would give as it is
/// (RULES.md D910): an element of an array held in a slot is read with no
/// copy of the array.
pub(super) fn argument<'a>(expr: &'a Expr, row: &'a Row, subqueries: &'a dyn Subqueries) -> Option<Result<&'a Value>> {
    match expr {
        Expr::Column(at) => Some(column(row, *at)),
        Expr::Const(value) => Some(Ok(value)),
        Expr::Outer(at, _) => subqueries.outer(*at).map(Ok),
        Expr::Dynamic(slot) => subqueries.slot(slot.index).filter(|value| held(value, &slot.data_type)).map(Ok),
        Expr::Neg(_)
        | Expr::Arith(..)
        | Expr::Call(..)
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
