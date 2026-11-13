//! [ISO/IEC 9075-2:2023 §6.1] DECFLOAT(precision): decimal floating point,
//! values of at most `precision` decimal digits [SR35] (`value::decfloat`).
//!
//! Its record bytes are its coefficient, 16 bytes, then its exponent, 4,
//! little-endian; its key the value's (`DecFloat::put_key`).

use std::cmp::Ordering;

use super::{Descriptor, SqlType, take};
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError, SqlState, fail_with};
use crate::value::cast::{NumericLiteral, numeric_parts, uncastable};
use crate::value::comparison::{compare_numbers, number_bucket};
use crate::value::{ArithOp, Bucket, DEFAULT_DECFLOAT_PRECISION, DataType, DecFloat, Value, assignment_rule, unassignable};

/// [ISO/IEC 9075-2:2023 §6.1] DECFLOAT, the name of `<decimal
/// floating-point type>`. Not a key word here, as no key word of SQL-92,
/// so a column of SQL-92 named DECFLOAT keeps its name.
pub const DECFLOAT_NAME: &str = "DECFLOAT";

/// The kind of DECFLOAT(precision).
#[derive(Debug)]
pub struct DecFloatKind;

/// The one value of it.
pub static DECFLOAT: DecFloatKind = DecFloatKind;

/// [§4.5.1] The radix of its precision: decimal digits.
const DECIMAL_RADIX: u32 = 10;
/// Bytes of a value in a record: its coefficient, then its exponent.
const RECORD_BYTES: usize = COEFFICIENT_BYTES + EXPONENT_BYTES;
/// Bytes of a record's coefficient.
const COEFFICIENT_BYTES: usize = 16;
/// Bytes of a record's exponent.
const EXPONENT_BYTES: usize = 4;

/// The precision of `t`, a DECFLOAT type.
fn precision_of(t: &DataType) -> u32 {
    if let DataType::DecFloat { precision } = t { *precision } else { DEFAULT_DECFLOAT_PRECISION }
}

/// A value not of a column of `t`'s type.
fn mismatch(v: &Value, t: &DataType) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] {v} is not a value of a column of type {t}"))
}

/// SQLSTATE 22003: `what` has no value of `t`, as the rule `rule` says.
fn out_of_range<T>(what: &dyn std::fmt::Display, t: &DataType, rule: &str) -> Result<T> {
    fail_with(Code::Data, SqlState::NUMERIC_VALUE_OUT_OF_RANGE, format!("[{rule}] {what} is outside the range of {t}"))
}

/// `v`, a number, as a DECFLOAT value of `precision` digits; none if it is
/// no number, or a number out of range.
fn decfloat_of(v: &Value, precision: u32) -> Option<DecFloat> {
    match v {
        Value::Exact(d) => DecFloat::of_decimal(d)?.rounded_to(precision),
        Value::Approx(x) => DecFloat::of_f64(*x, precision),
        Value::DecFloat(d) => d.rounded_to(precision),
        Value::Null
        | Value::Char(_)
        | Value::Binary(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Array(_)
        | Value::Row(_)
        | Value::Extension(_) => None,
    }
}

/// [ISO/IEC 9075-2:2023 §6.13 GR10b] The value of `lit`, a `<signed
/// numeric literal>`, as DECFLOAT(`precision`); none if out of range.
fn of_literal(lit: &NumericLiteral, precision: u32) -> Option<DecFloat> {
    let e = lit.exponent.unwrap_or(0).saturating_sub(i64::try_from(lit.fraction).ok()?);
    DecFloat::of_digits(lit.negative, &lit.digits, e, precision)
}

/// [ISO/IEC 9075-2:2023 §6.30 GR4, GR7] `op` applied to `a` and `b`,
/// numbers, one at least DECFLOAT: each as DECFLOAT, the result of
/// `MAX_DECFLOAT_PRECISION` digits (IV134).
///
/// # Errors
///
/// SQLSTATE 22012 for a divisor of 0; 22003 for a result, or an
/// approximate operand, out of range.
pub(crate) fn arith(op: ArithOp, left: &Value, right: &Value) -> Result<Value> {
    let precision = crate::value::MAX_DECFLOAT_PRECISION;
    let (Some(first), Some(second)) = (decfloat_of(left, precision), decfloat_of(right, precision)) else {
        let target = DataType::DecFloat { precision };
        return out_of_range(&format!("{left} {} {right}", op.symbol()), &target, "ISO/IEC 9075-2:2023 §6.13 GR10a ii");
    };
    if op == ArithOp::Div && second.is_zero() {
        return fail_with(Code::Data, SqlState::DIVISION_BY_ZERO, "[ISO/IEC 9075-2:2023 §6.30 GR4] a divisor shall not be 0");
    }
    let result = match op {
        ArithOp::Add => first.plus(second),
        ArithOp::Sub => first.minus(second),
        ArithOp::Mul => first.times(second),
        ArithOp::Div => first.over(second),
    };
    result.map(Value::DecFloat).map_or_else(
        || {
            fail_with(
                Code::Data,
                SqlState::NUMERIC_VALUE_OUT_OF_RANGE,
                format!("[ISO/IEC 9075-2:2023 §6.30 GR7] {first} {} {second} has an exponent outside the range of DECFLOAT", op.symbol()),
            )
        },
        Ok,
    )
}

impl SqlType for DecFloatKind {
    fn name(&self) -> &'static str {
        DECFLOAT_NAME
    }

    fn written(&self, t: &DataType) -> String {
        format!("{DECFLOAT_NAME}({})", precision_of(t))
    }

    /// [ISO/IEC 9075-2:2023 §4.5.1] Any two numbers are comparable.
    fn comparable(&self, other: &DataType) -> bool {
        other.is_numeric()
    }

    fn compare(&self, a: &Value, b: &Value) -> Option<Ordering> {
        compare_numbers(a, b)
    }

    fn bucket(&self, v: &Value) -> Option<Bucket> {
        number_bucket(v)
    }

    /// [ISO/IEC 9075-2:2023 §9.2 GR3b xv] A number rounded to the
    /// precision, half to even, if it is in range.
    fn assign(&self, v: Value, t: &DataType, rule: &str) -> Result<Value> {
        if !matches!(v, Value::Exact(_) | Value::Approx(_) | Value::DecFloat(_)) {
            return unassignable(&v, t, rule);
        }
        match decfloat_of(&v, precision_of(t)) {
            Some(d) => Ok(Value::DecFloat(d)),
            None => out_of_range(&v, t, &assignment_rule(rule, "ISO/IEC 9075-2:2023 §9.2 GR3b xv 3")),
        }
    }

    /// [ISO/IEC 9075-2:2023 §6.13 GR10] A number rounded to the precision
    /// \[GR10a\]; a string's `<signed numeric literal>`, its spaces trimmed,
    /// cast \[GR10b\].
    fn cast(&self, v: Value, t: &DataType) -> Result<Value> {
        let precision = precision_of(t);
        match v {
            Value::Null => Ok(Value::Null),
            Value::Char(ref s) => {
                let Some(lit) = numeric_parts(s.trim_matches(' ')) else {
                    return fail_with(
                        Code::Data,
                        SqlState::INVALID_CHARACTER_VALUE_FOR_CAST,
                        format!("[ISO/IEC 9075-2:2023 §6.13 GR10b i] '{s}' is not a <signed numeric literal>"),
                    );
                };
                of_literal(&lit, precision)
                    .map_or_else(|| out_of_range(&format!("'{s}'"), t, "ISO/IEC 9075-2:2023 §6.13 GR10a ii"), |d| Ok(Value::DecFloat(d)))
            }
            Value::Exact(_) | Value::Approx(_) | Value::DecFloat(_) => decfloat_of(&v, precision)
                .map_or_else(|| out_of_range(&v, t, "ISO/IEC 9075-2:2023 §6.13 GR10a ii"), |d| Ok(Value::DecFloat(d))),
            Value::Binary(_)
            | Value::Bool(_)
            | Value::Datetime(_)
            | Value::Interval(_)
            | Value::Array(_)
            | Value::Row(_)
            | Value::Extension(_) => uncastable(&v, t),
        }
    }

    /// No literal is of DECFLOAT [§4.5.2, IV119], so a CAST of its text.
    fn literal(&self, v: &Value) -> String {
        format!("CAST('{v}' AS {DECFLOAT_NAME})")
    }

    fn text(&self, v: &Value) -> String {
        v.to_string()
    }

    fn position(&self, v: &Value) -> Option<f64> {
        if let Value::DecFloat(d) = v { Some(d.to_f64()).filter(|x| x.is_finite()) } else { None }
    }

    /// [ISO/IEC 9075-2:2023 §6.1 GR18] DECFLOAT, of its decimal precision.
    fn descriptor(&self, t: &DataType) -> Descriptor {
        Descriptor { precision: Some(precision_of(t)), radix: Some(DECIMAL_RADIX), ..Descriptor::of(DECFLOAT_NAME) }
    }

    fn put_value(&self, v: &Value, out: &mut Vec<u8>) {
        let d = if let Value::DecFloat(d) = v { *d } else { DecFloat::ZERO };
        out.put_i128(d.coefficient());
        out.put_u32(u32::from_le_bytes(d.exponent().to_le_bytes()));
    }

    fn value(&self, r: &mut Reader<'_>) -> Result<Value> {
        let coefficient = r.i128()?;
        let exponent = i32::from_le_bytes(r.u32()?.to_le_bytes());
        decoded(coefficient, exponent).ok_or_else(|| r.corrupt("a DECFLOAT value is malformed"))
    }

    fn put_params(&self, t: &DataType, out: &mut Vec<u8>) {
        out.put_u32(precision_of(t));
    }

    fn params(&self, r: &mut Reader<'_>) -> Result<DataType> {
        Ok(DataType::DecFloat { precision: r.u32()? })
    }

    fn record_width(&self, _: &DataType) -> Option<usize> {
        Some(RECORD_BYTES)
    }

    fn encode_record(&self, v: &Value, t: &DataType) -> Result<Vec<u8>> {
        let Value::DecFloat(d) = v else { return Err(mismatch(v, t)) };
        Ok([d.coefficient().to_le_bytes().as_slice(), d.exponent().to_le_bytes().as_slice()].concat())
    }

    fn decode_record(&self, field: &[u8], _: &DataType) -> Option<Value> {
        let (c, e) = field.split_at_checked(COEFFICIENT_BYTES)?;
        decoded(i128::from_le_bytes(c.try_into().ok()?), i32::from_le_bytes(e.try_into().ok()?))
    }

    fn encode_key(&self, v: &Value, t: &DataType, out: &mut Vec<u8>) -> Result<()> {
        let Value::DecFloat(d) = v else { return Err(mismatch(v, t)) };
        d.put_key(out);
        Ok(())
    }

    fn decode_key(&self, rest: &mut &[u8], _: &DataType) -> Result<Value> {
        let before = *rest;
        let mut scan = before;
        let d = DecFloat::take_key(&mut scan).ok_or_else(super::malformed)?;
        take(rest, before.len().saturating_sub(scan.len()))?;
        Ok(Value::DecFloat(d))
    }

    /// Itself, of an exact number too, which a DECFLOAT value holds
    /// exactly; none of an approximate one.
    fn anchor(&self, v: &Value, _: &DataType) -> Option<(Value, bool)> {
        match v {
            Value::DecFloat(_) => Some((v.clone(), true)),
            Value::Exact(d) => Some((Value::DecFloat(DecFloat::of_decimal(d)?), true)),
            Value::Null
            | Value::Char(_)
            | Value::Approx(_)
            | Value::Binary(_)
            | Value::Bool(_)
            | Value::Datetime(_)
            | Value::Interval(_)
            | Value::Array(_)
            | Value::Row(_)
            | Value::Extension(_) => None,
        }
    }
}

/// The value of `coefficient` and `exponent` read back, if it is one in
/// its one form.
fn decoded(coefficient: i128, exponent: i32) -> Option<Value> {
    let d = DecFloat::new(coefficient, i64::from(exponent), crate::value::MAX_DECFLOAT_PRECISION)?;
    (d.coefficient() == coefficient && d.exponent() == exponent).then_some(Value::DecFloat(d))
}
