//! \[DESIGN\] What bounds the values of a set function (RULES.md D1060,
//! D15). SUM judges its sum once, as it is finished, against
//! `MAX_EXACT_PRECISION` digits, and keeps it on the way in 128 bits
//! (`functions/sum.rs`). A statement reads fewer than 2^63 rows, the bound
//! COUNT rests on (RULES.md D552), which is fewer than 10^`ROW_DIGITS`. So
//! the sum of exact values of precision p has at most p + `ROW_DIGITS`
//! digits, at their scale, and while those are within
//! `MAX_EXACT_PRECISION` it raises nothing, on the way or finished: 10^38
//! is less than 2^127. AVG keeps the same sum, and its mean is within its
//! values' digits before the point with `AVG_EXTRA_SCALE` more after it
//! [X3.135-1986 §5.8 SR9c], so it raises nothing either. COUNT, MAX and
//! MIN raise nothing of their own.

use super::{Types, exact_shape};
use crate::functions::{AVG_EXTRA_SCALE, ColumnarKind};
use crate::logical::AggCall;
use crate::value::{DataType, MAX_EXACT_PRECISION, Spelled};

/// The decimal digits that count the rows a statement reads, fewer than
/// 2^63, which is 9223372036854775808, a number of 19 digits.
pub const ROW_DIGITS: u32 = 19;

/// The precision and scale the argument of `a` is within, if exact and
/// bounded.
fn argument(a: &AggCall, types: Types<'_>, depth: usize) -> Option<(u32, u32)> {
    exact_shape(a.arg.as_ref()?, types, depth)
}

/// Whether `a`, a SUM or an AVG, is of exact values whose sum is always
/// within `MAX_EXACT_PRECISION` digits, so the function itself raises
/// nothing, whatever rows it is given; `types` gives the declared types
/// of its input's columns. Its argument and filter are judged apart.
pub fn total_fits(a: &AggCall, types: Types<'_>) -> bool {
    argument(a, types, 0).is_some_and(|(p, _)| p.saturating_add(ROW_DIGITS) <= MAX_EXACT_PRECISION)
}

/// The type the values of the set function `a` are within, if known
/// (`result_type`), `types` giving the declared types of its input's
/// columns.
pub fn set_function_type(a: &AggCall, types: Types<'_>) -> Option<DataType> {
    result_type(a, types, 0)
}

/// The type the values of `a` are within, if known: of SUM and AVG of
/// bounded exact values, as the module says, capped at the result's; of
/// MAX and MIN, their argument's; of COUNT, its result's.
pub(super) fn result_type(a: &AggCall, types: Types<'_>, depth: usize) -> Option<DataType> {
    let exact = |(precision, scale): (u32, u32)| {
        let fits = precision <= MAX_EXACT_PRECISION && scale <= precision;
        fits.then_some(DataType::Exact { precision, scale, spelled: Spelled::Numeric })
    };
    match a.func.0.columnar()?.0 {
        ColumnarKind::Count => Some(a.result.clone()),
        ColumnarKind::Sum => {
            let (p, s) = argument(a, types, depth)?;
            exact((p.saturating_add(ROW_DIGITS), s)).or_else(|| Some(a.result.clone()))
        }
        ColumnarKind::Avg => {
            let (p, s) = argument(a, types, depth)?;
            exact((p.saturating_add(AVG_EXTRA_SCALE), s.saturating_add(AVG_EXTRA_SCALE))).or_else(|| Some(a.result.clone()))
        }
        ColumnarKind::Min | ColumnarKind::Max => match a.arg.as_ref()? {
            crate::logical::Expr::Column(c) => types(*c),
            other => exact(exact_shape(other, types, depth)?),
        },
    }
}

// Unit tests: the sums that fit, and a grouping's columns' types (RULES.md
// R-TEST-1).
#[cfg(test)]
mod tests;
