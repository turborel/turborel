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
        let mut kept = Vec::with_capacity(rows.len());
        let mut args = self.args;
        // Resolve vectors once, rather than materializing an entire row
        // and cloning its arguments again for every call.
        let columns: Vec<_> = self.columns.iter().map(|&(arg, column)| (arg, batch.values(column))).collect();
        for &row in rows {
            for &(arg, values) in &columns {
                if let (Some(slot), Some(value)) = (args.get_mut(arg), values.get(row)) {
                    *slot = value;
                }
            }
            let compared = self.function.0.call_borrowed(&args, temp).and_then(|value| {
                // Reading either large object can fail; keep the written
                // operand order as well as the comparison's truth (D15).
                let (left, right) = if self.reversed { (self.comparison.1, &value) } else { (&value, self.comparison.1) };
                compared_read(left, self.comparison.0, right)
            });
            match compared {
                Ok(Truth::True) => kept.push(row),
                Ok(Truth::False | Truth::Unknown) => {}
                Err(error) => return (kept, Some(error)),
            }
        }
        (kept, None)
    }
}

#[cfg(test)]
mod tests;
