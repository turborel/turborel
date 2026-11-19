//! Evaluate a scalar comparison directly over batch columns. Constant
//! arguments are retained for the batch; only column arguments change.
//! Calls run in the same row order as the general evaluator, stopping at
//! the first error and preserving the successful prefix (D15, D31).

use crate::error::SqlError;
use crate::exec::eval::compared_read;
use crate::functions::ScalarRef;
use crate::logical::{Cond, Expr, Truth};
use crate::storage::Batch;
use crate::value::lob::TempLobs;
use crate::value::{CompOp, Value};

pub(super) struct Call<'a> {
    function: &'a ScalarRef,
    args: Vec<&'a Value>,
    columns: Vec<(usize, usize)>,
    comparison: (CompOp, &'a Value),
    reversed: bool,
}

impl<'a> Call<'a> {
    pub(super) fn of(cond: &'a Cond, columns: &[usize]) -> Option<Self> {
        let Cond::Compare(left, op, right) = cond else { return None };
        let (function, args, constant, reversed) = match (left, right) {
            (Expr::Call(function, args), Expr::Const(value)) => (function, args, value, false),
            (Expr::Const(value), Expr::Call(function, args)) => (function, args, value, true),
            _ => return None,
        };
        // One that reads its statement is evaluated a row at a time (D87).
        if function.0.uses_context(args.len()) {
            return None;
        }
        let mut call = Self { function, args: Vec::with_capacity(args.len()), columns: Vec::new(), comparison: (*op, constant), reversed };
        for (position, arg) in args.iter().enumerate() {
            match arg {
                Expr::Const(value) => call.args.push(value),
                Expr::Column(column) => {
                    let at = columns.iter().position(|c| c == column)?;
                    call.columns.push((position, at));
                    call.args.push(&Value::Null);
                }
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
                | Expr::Subquery(..) => return None,
            }
        }
        Some(call)
    }

    pub(super) fn in_order(self, batch: &Batch, rows: &[usize], temp: Option<&dyn TempLobs>) -> (Vec<usize>, Option<SqlError>) {
        let Call { function, mut args, columns, comparison: (op, constant), reversed } = self;
        let mut kept = Vec::with_capacity(rows.len());
        // Resolve vectors once, rather than materializing an entire row
        // and cloning its arguments again for every call.
        let columns: Vec<_> = columns.iter().map(|&(arg, column)| (arg, batch.values(column))).collect();
        let turned = if reversed { op.flip() } else { op };
        for &row in rows {
            for &(arg, values) in &columns {
                if let (Some(slot), Some(value)) = (args.get_mut(arg), values.get(row)) {
                    *slot = value;
                }
            }
            let compared =
                decided(function, &args, turned, constant).unwrap_or_else(|| measured(function, &args, (op, constant), reversed, temp));
            match compared {
                Ok(Truth::True) => kept.push(row),
                Ok(Truth::False | Truth::Unknown) => {}
                Err(error) => return (kept, Some(error)),
            }
        }
        (kept, None)
    }
}

/// "`function(args) op value`", or the constant first if `reversed`, by
/// the call's value. Reading either large object can fail; the written
/// operand order is kept, as is the comparison's truth (D15).
fn measured(
    function: &ScalarRef,
    args: &[&Value],
    (op, value): (CompOp, &Value),
    reversed: bool,
    temp: Option<&dyn TempLobs>,
) -> Result<Truth, SqlError> {
    let made = function.0.call_borrowed(args, temp)?;
    let (left, right) = if reversed { (value, &made) } else { (&made, value) };
    compared_read(left, op, right)
}

/// "`function(args) op value`" decided by the function without the call's
/// value, where it does (`ScalarFunction::call_compared`), `op` turned
/// where the constant is written first; none to make the call and compare
/// its value.
fn decided(function: &ScalarRef, args: &[&Value], op: CompOp, value: &Value) -> Option<Result<Truth, SqlError>> {
    let truth = |holds: Option<bool>| holds.map_or(Truth::Unknown, |holds| if holds { Truth::True } else { Truth::False });
    Some(function.0.call_compared(args, op, value)?.map(truth))
}

#[cfg(test)]
mod tests;
