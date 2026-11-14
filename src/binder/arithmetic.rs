//! [X3.135-1986 §5.9; ISO/IEC 9075-2:2023 §6.30] Dyadic arithmetic: its
//! operands bound, a `?` among them typed by the other [ISO/IEC 9075:1992
//! §17.6 GR5f-h], and the type of its result.

use super::expression::dynamic_of;
use super::interval::assumed;
use super::{Binder, Scope, rule};
use crate::ast::ValueExpression;
use crate::error::{Code, Result, SqlError};
use crate::logical::Expr;
use crate::params::ParameterType;
use crate::value::{ArithOp, DOUBLE_PRECISION, DataType, MAX_DECFLOAT_PRECISION, MAX_EXACT_PRECISION, Spelled};

/// [X3.135-1986 §5.9 SR4] The type of an operation on two exact numbers. Its
/// precision is implementor-defined: `MAX_EXACT_PRECISION`. Its scale is
/// max(s1, s2) for + and - \[SR4b\], s1 + s2 for * \[SR4c\], and for / at least
/// max(s1, s2): a quotient keeps as many more fractional digits as fit, so
/// as to lose no leading digit \[GR6b\], the implementor-defined choice of
/// \[SR4d\] (`Decimal::checked_div`).
///
/// A scale above the precision is not a type [X3.135-1986 §5.5 SR4], so a
/// product whose scale would exceed `MAX_EXACT_PRECISION` is valid SQL-86 that
/// this implementation does not process: SQLCODE -900. Capping the scale
/// instead made every row fail at run time.
fn exact_result_type(op: ArithOp, s1: u32, s2: u32) -> Result<DataType> {
    let scale = match op {
        ArithOp::Mul => s1.checked_add(s2).filter(|s| *s <= MAX_EXACT_PRECISION).ok_or_else(|| {
            SqlError::new(
                Code::Unsupported,
                format!(
                    "[X3.135-1986 §5.9 SR4c, §5.5 SR4] a product of scales {s1} and {s2} needs a scale above this implementation's maximum precision of {MAX_EXACT_PRECISION}"
                ),
            )
        })?,
        ArithOp::Add | ArithOp::Sub | ArithOp::Div => s1.max(s2),
    };
    Ok(DataType::Exact { precision: MAX_EXACT_PRECISION, scale, spelled: Spelled::Numeric })
}

impl Binder<'_> {
    /// [X3.135-1986 §5.9 SR4, SR5] a dyadic operator and the type of its result.
    pub(super) fn dyadic(
        &self,
        left: &ValueExpression,
        op: ArithOp,
        right: &ValueExpression,
        scope: Scope<'_>,
    ) -> Result<(Expr, DataType)> {
        // [ISO/IEC 9075:1992 §17.6 GR5f-h] a `?` takes the type its other
        // operand gives it. Without one, each is bound as it is, so a chain
        // of operators nests no deeper in the stack than it did.
        let dynamic = dynamic_of(left).is_some() || dynamic_of(right).is_some();
        let operands = if dynamic {
            self.operands((left, right), op, scope)?
        } else {
            [self.value_expression(left, scope)?, self.value_expression(right, scope)?]
        };
        self.arithmetic(operands, op, dynamic)
    }

    /// The operation `op` of its operands, bound, typed: of `dynamic`
    /// operands, `?`s among them. Apart from `dyadic`, so none of its
    /// locals is in the frame a chain of operators recurs through
    /// (RULES.md R-CF-6).
    fn arithmetic(&self, [(a, left_type), (b, right_type)]: [(Expr, DataType); 2], op: ArithOp, dynamic: bool) -> Result<(Expr, DataType)> {
        if left_type.is_character() || right_type.is_character() {
            return Err(rule("[X3.135-1986 §5.9 SR3] character string values cannot be used with arithmetic operators"));
        }
        if left_type.is_binary() || right_type.is_binary() {
            return Err(rule("[ISO/IEC 9075-2:2023 §6.30 SR3] binary string values cannot be used with arithmetic operators"));
        }
        distinct_operand(&left_type)?;
        distinct_operand(&right_type)?;
        // [§6.14, §6.15] datetimes and intervals.
        if left_type.is_datetime() || right_type.is_datetime() || left_type.is_interval() || right_type.is_interval() {
            return self.temporal((a, left_type), op, (b, right_type), dynamic);
        }
        let t = match (left_type, right_type) {
            (DataType::Exact { scale: s1, .. }, DataType::Exact { scale: s2, .. }) => exact_result_type(op, s1, s2)?,
            // [ISO/IEC 9075-2:2023 §6.30 SR1a] DECFLOAT of 34 digits (IV134).
            (DataType::DecFloat { .. }, _) | (_, DataType::DecFloat { .. }) => DataType::DecFloat { precision: MAX_DECFLOAT_PRECISION },
            // [X3.135-1986 §5.9 SR5]
            _ => DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::Float },
        };
        Ok((Expr::Arith(Box::new(a), op, Box::new(b)), t))
    }

    /// [ISO/IEC 9075:1992 §17.6 GR5f-h] The two operands of `op` bound: a
    /// `<dynamic parameter specification>` among them taking the type
    /// `interval::assumed` gives. Both of them one takes none \[GR4c\].
    fn operands(&self, (a, b): (&ValueExpression, &ValueExpression), op: ArithOp, scope: Scope<'_>) -> Result<[(Expr, DataType); 2]> {
        match (dynamic_of(a), dynamic_of(b)) {
            (Some(_), None) => {
                let y = self.value_expression(b, scope)?;
                Ok([self.typed(a, ParameterType::Of(assumed(op, y.1.clone(), true)), scope)?, y])
            }
            (None, Some(_)) => {
                let x = self.value_expression(a, scope)?;
                let y = self.typed(b, ParameterType::Of(assumed(op, x.1.clone(), false)), scope)?;
                Ok([x, y])
            }
            (Some(_), Some(_)) | (None, None) => self.pair((a, b), scope),
        }
    }
}

/// [ISO/IEC 9075-2:2023 §6.30 SR3, §4.9.5] That `t`, an arithmetic
/// operand's type, is no distinct type: its values are of no numeric type,
/// whatever its source, and are operands of comparison alone, or of the
/// functions its definition makes (`binder/routines.rs`).
pub(super) fn distinct_operand(t: &DataType) -> Result<()> {
    if let DataType::Distinct(_) = t {
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §6.30 SR3] a <numeric primary> is numeric, not of the distinct type {t}")));
    }
    Ok(())
}

// Unit tests: the exact result types of [X3.135-1986 §5.9 SR4] (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [X3.135-1986 §5.9 SR4b, SR4c, §5.5 SR4] product scale is s1 + s2 up to
    /// `MAX_EXACT_PRECISION` and rejected beyond it; sums keep max(s1, s2).
    #[test]
    fn exact_result_scales() {
        let half = 9_u32; // any split of MAX_EXACT_PRECISION into two scales
        let scale = |op, s1, s2| {
            exact_result_type(op, s1, s2)
                .map(|t| if let DataType::Exact { scale, .. } = t { Some(scale) } else { None })
                .map_err(|e| e.code)
        };
        assert_eq!(scale(ArithOp::Mul, half, MAX_EXACT_PRECISION - half), Ok(Some(MAX_EXACT_PRECISION)));
        assert_eq!(scale(ArithOp::Mul, half, MAX_EXACT_PRECISION - half + 1), Err(Code::Unsupported));
        assert_eq!(scale(ArithOp::Add, MAX_EXACT_PRECISION, MAX_EXACT_PRECISION), Ok(Some(MAX_EXACT_PRECISION)));
        assert_eq!(scale(ArithOp::Sub, 2, 5), Ok(Some(5)));
        assert_eq!(scale(ArithOp::Div, 5, 2), Ok(Some(5)));
    }
}
