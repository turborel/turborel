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
use crate::value::{ArithOp, DOUBLE_PRECISION, DataType, MAX_DECFLOAT_PRECISION, MAX_EXACT_PRECISION, Spelled, bound_of};

/// [X3.135-1986 §5.9 SR4; ISO/IEC 9075-2:2023 §6.30 SR1c] The type of an
/// operation on two exact numbers, of types `(p1, s1)` and `(p2, s2)`.
/// Its scale is max(s1, s2) for + and - \[SR4b\], s1 + s2 for * \[SR4c\],
/// and for / at least max(s1, s2): a quotient keeps as many more
/// fractional digits as fit, so as to lose no leading digit \[GR6b\], the
/// implementor-defined choice of \[SR4d\] (`Decimal::checked_div`). Its
/// precision is implementation-defined (IV136): \[DESIGN\] (RULES.md
/// D696) the digits every value of the operands' types gives
/// (`value::bound_of`), up to `MAX_EXACT_PRECISION`, and of a quotient
/// `MAX_EXACT_PRECISION`; so a sum of two columns of 18 digits is of 19,
/// stored as such, and a value past the precision raises [GR6a].
///
/// A scale above the precision is not a type [X3.135-1986 §5.5 SR4], so a
/// product whose scale would exceed `MAX_EXACT_PRECISION` is valid SQL-86 that
/// this implementation does not process: SQLCODE -900. Capping the scale
/// instead made every row fail at run time.
fn exact_result_type(op: ArithOp, (p1, s1): (u32, u32), (p2, s2): (u32, u32)) -> Result<DataType> {
    let (precision, scale) = bound_of(op, (p1, s1), (p2, s2)).unwrap_or((MAX_EXACT_PRECISION, s1.max(s2)));
    if scale > MAX_EXACT_PRECISION {
        return Err(SqlError::new(
            Code::Unsupported,
            format!(
                "[X3.135-1986 §5.9 SR4c, §5.5 SR4] a product of scales {s1} and {s2} needs a scale above this implementation's maximum precision of {MAX_EXACT_PRECISION}"
            ),
        ));
    }
    Ok(DataType::Exact { precision: precision.min(MAX_EXACT_PRECISION), scale, spelled: Spelled::Numeric })
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
    fn arithmetic(&self, [left, right]: [(Expr, DataType); 2], op: ArithOp, dynamic: bool) -> Result<(Expr, DataType)> {
        // [EXTENSION] of vectors, a function of VECTOR (RULES.md D93).
        if let Some(vectors) = self.vector_arithmetic(&left, op, &right) {
            return vectors;
        }
        let ((a, left_type), (b, right_type)) = (left, right);
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
            (DataType::Exact { precision: p1, scale: s1, .. }, DataType::Exact { precision: p2, scale: s2, .. }) => {
                exact_result_type(op, (p1, s1), (p2, s2))?
            }
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

    fn shape(op: ArithOp, a: (u32, u32), b: (u32, u32)) -> std::result::Result<(u32, u32), Code> {
        match exact_result_type(op, a, b) {
            Ok(DataType::Exact { precision, scale, .. }) => Ok((precision, scale)),
            Ok(_) => Err(Code::Api),
            Err(e) => Err(e.code),
        }
    }

    /// Verifies: [X3.135-1986 §5.9 SR4b, SR4c, §5.5 SR4] product scale is s1 + s2 up to
    /// `MAX_EXACT_PRECISION` and rejected beyond it; sums keep max(s1, s2).
    #[test]
    fn exact_result_scales() {
        let half = 19_u32; // any split of MAX_EXACT_PRECISION into two scales
        let max = MAX_EXACT_PRECISION;
        assert_eq!(shape(ArithOp::Mul, (half, half), (max - half, max - half)), Ok((max, max)));
        assert_eq!(shape(ArithOp::Mul, (half, half), (max, max - half + 1)), Err(Code::Unsupported));
        assert_eq!(shape(ArithOp::Add, (max, max), (max, max)), Ok((max, max)));
        assert_eq!(shape(ArithOp::Sub, (9, 2), (9, 5)), Ok((13, 5)));
        assert_eq!(shape(ArithOp::Div, (9, 5), (9, 2)), Ok((max, 5)));
    }

    /// Verifies: [ISO/IEC 9075-2:2023 §6.30 SR1c ii, iii] (IV136) the
    /// precision of a sum, difference or product is the digits its
    /// operands' types give, up to 38: two DECIMAL(15,2) sum to 16 digits
    /// and multiply to 30, two INTEGERs to 11 and 20, and two of 30 digits
    /// to 38 either way.
    #[test]
    fn exact_result_precisions() {
        assert_eq!(shape(ArithOp::Add, (15, 2), (15, 2)), Ok((16, 2)));
        assert_eq!(shape(ArithOp::Sub, (15, 2), (10, 0)), Ok((16, 2)));
        assert_eq!(shape(ArithOp::Mul, (15, 2), (15, 2)), Ok((30, 4)));
        assert_eq!(shape(ArithOp::Add, (10, 0), (10, 0)), Ok((11, 0)));
        assert_eq!(shape(ArithOp::Mul, (10, 0), (10, 0)), Ok((20, 0)));
        assert_eq!(shape(ArithOp::Add, (30, 0), (30, 0)), Ok((31, 0)));
        assert_eq!(shape(ArithOp::Mul, (30, 0), (30, 0)), Ok((MAX_EXACT_PRECISION, 0)));
        assert_eq!(shape(ArithOp::Add, (MAX_EXACT_PRECISION, 0), (1, 0)), Ok((MAX_EXACT_PRECISION, 0)));
    }
}
