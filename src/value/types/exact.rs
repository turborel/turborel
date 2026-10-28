//! [X3.135-1986 §5.5] NUMERIC(precision, scale), and DECIMAL, INTEGER and
//! SMALLINT, which are exact numbers of it.
//!
//! Its record bytes and its key are the units at the column's scale, a
//! 64-bit integer, which holds `MAX_EXACT_PRECISION` (18) digits: little-
//! endian in a record; in a key big-endian, its sign bit flipped, so the
//! bytes order as the numbers do.

use std::cmp::Ordering;

use super::{Descriptor, NUMBER_BYTES, SIGN, SqlType, take};
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError, SqlState, fail_with};
use crate::value::cast::{approximate_to_exact, numeric_literal, uncastable};
use crate::value::comparison::{compare_numbers, number_bucket};
use crate::value::{Bucket, DataType, Decimal, Value, pow10, unassignable};

/// The kind of NUMERIC(precision, scale).
#[derive(Debug)]
pub struct Exact;

/// The one value of it.
pub static EXACT: Exact = Exact;

/// [§21.3.5 Description 3] The radix of its precision: decimal digits.
const DECIMAL_RADIX: u32 = 10;

/// The precision and scale of `t`, an exact numeric type.
fn shape(t: &DataType) -> (u32, u32) {
    if let DataType::Exact { precision, scale } = t { (*precision, *scale) } else { (0, 0) }
}

/// A value not of a column of `t`'s type.
fn mismatch(v: &Value, t: &DataType) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] {v} is not a value of a column of type {t}"))
}

/// The units of `value` at `scale`, if they are whole there and 64 bits
/// hold them: as a column of that scale stores them.
///
/// # Errors
///
/// SQLCODE -910 if `value` has more digits after the point than `scale`
/// keeps, or more digits in all than 64 bits hold: a value not of the
/// column's type.
pub(crate) fn units_at(value: &Decimal, scale: u32) -> Result<i64> {
    let units = if value.scale <= scale {
        pow10(scale.saturating_sub(value.scale)).and_then(|f| value.units.checked_mul(f))
    } else {
        pow10(value.scale.saturating_sub(scale)).filter(|&f| value.units.checked_rem(f) == Some(0)).and_then(|f| value.units.checked_div(f))
    };
    units.and_then(|u| i64::try_from(u).ok()).ok_or_else(|| {
        SqlError::new(Code::Api, format!("[DESIGN] {value} cannot be stored at scale {scale} in 64 bits: it is not of the column's type"))
    })
}

/// The units of the greatest number at `scale` not above `d`, within 64
/// bits.
fn floor_units(d: &Decimal, scale: u32) -> i64 {
    let clamp = |negative: bool| if negative { i64::MIN } else { i64::MAX };
    let units = if d.scale <= scale {
        pow10(scale.saturating_sub(d.scale)).and_then(|f| d.units.checked_mul(f))
    } else {
        // Divided by more than 10^38, a value of 38 digits is below 1.
        Some(pow10(d.scale.saturating_sub(scale)).map_or(if d.units < 0 { -1 } else { 0 }, |f| d.units.div_euclid(f)))
    };
    units.map_or_else(|| clamp(d.units < 0), |u| i64::try_from(u).unwrap_or_else(|_| clamp(u < 0)))
}

impl SqlType for Exact {
    fn name(&self) -> &'static str {
        "NUMERIC"
    }

    fn written(&self, t: &DataType) -> String {
        let (precision, scale) = shape(t);
        format!("NUMERIC({precision},{scale})")
    }

    /// [X3.135-1986 §4.2] Numbers are comparable, exact and approximate.
    fn comparable(&self, other: &DataType) -> bool {
        matches!(other, DataType::Exact { .. } | DataType::Approximate { .. })
    }

    fn compare(&self, a: &Value, b: &Value) -> Option<Ordering> {
        compare_numbers(a, b)
    }

    fn bucket(&self, v: &Value) -> Option<Bucket> {
        number_bucket(v)
    }

    /// [ISO/IEC 9075:1992 §9.2 GR3k] Its extra fraction digits truncated,
    /// which is implementation-defined; an approximate number rounded.
    fn assign(&self, v: Value, t: &DataType, rule: &str) -> Result<Value> {
        let (precision, scale) = shape(t);
        match v {
            Value::Exact(d) => {
                let units = if d.scale > scale {
                    d.scale.checked_sub(scale).and_then(pow10).and_then(|p| d.units.checked_div(p))
                } else {
                    scale.checked_sub(d.scale).and_then(pow10).and_then(|p| d.units.checked_mul(p))
                };
                // The divisor 10^n is at least 1, and the quotient smaller than `units`.
                match units.map(|u| Decimal::new(u, scale)).filter(|out| out.fits(precision, scale)) {
                    Some(out) => Ok(Value::Exact(out)),
                    None => fail_with(
                        Code::Data,
                        SqlState::NUMERIC_VALUE_OUT_OF_RANGE,
                        format!("[{rule}; ISO/IEC 9075:1992 §9.2 GR3k] {d} does not fit {t} without losing leading digits"),
                    ),
                }
            }
            Value::Approx(x) => approximate_to_exact(x, (precision, scale), rule),
            Value::Null | Value::Char(_) | Value::Bits(_) | Value::Datetime(_) | Value::Extension(_) => unassignable(&v, t, rule),
        }
    }

    /// [ISO/IEC 9075:1992 §6.10 GR3] A string's `<signed numeric literal>`,
    /// its spaces trimmed, cast \[GR3b\]; a number's extra digits truncated,
    /// as store assignment does, an approximate one rounded \[GR3a,
    /// implementation-defined\].
    fn cast(&self, v: Value, t: &DataType) -> Result<Value> {
        match v {
            Value::Null => Ok(Value::Null),
            Value::Char(s) => self.cast(numeric_literal(&s)?, t),
            Value::Exact(_) => self.assign(v, t, "ISO/IEC 9075:1992 §6.10 GR3a"),
            Value::Approx(x) => approximate_to_exact(x, shape(t), "ISO/IEC 9075:1992 §6.10 GR3a ii"),
            Value::Bits(_) | Value::Datetime(_) | Value::Extension(_) => uncastable(&v, t),
        }
    }

    fn literal(&self, v: &Value) -> String {
        v.to_string()
    }

    fn text(&self, v: &Value) -> String {
        v.to_string()
    }

    fn position(&self, v: &Value) -> Option<f64> {
        if let Value::Exact(d) = v { Some(d.to_f64()) } else { None }
    }

    /// NUMERIC, of decimal precision [ISO/IEC 9075:1992 §6.1 SR17].
    fn descriptor(&self, t: &DataType) -> Descriptor {
        let (precision, scale) = shape(t);
        Descriptor { precision: Some(precision), radix: Some(DECIMAL_RADIX), scale: Some(scale), ..Descriptor::of(self.name()) }
    }

    fn put_value(&self, v: &Value, out: &mut Vec<u8>) {
        let d = if let Value::Exact(d) = v { *d } else { Decimal::new(0, 0) };
        out.put_i128(d.units);
        out.put_u32(d.scale);
    }

    fn value(&self, r: &mut Reader<'_>) -> Result<Value> {
        let units = r.i128()?;
        Ok(Value::Exact(Decimal::new(units, r.u32()?)))
    }

    fn put_params(&self, t: &DataType, out: &mut Vec<u8>) {
        let (precision, scale) = shape(t);
        out.put_u32(precision);
        out.put_u32(scale);
    }

    fn params(&self, r: &mut Reader<'_>) -> Result<DataType> {
        Ok(DataType::Exact { precision: r.u32()?, scale: r.u32()? })
    }

    fn record_width(&self, _: &DataType) -> Option<usize> {
        Some(NUMBER_BYTES)
    }

    fn encode_record(&self, v: &Value, t: &DataType) -> Result<Vec<u8>> {
        let Value::Exact(d) = v else { return Err(mismatch(v, t)) };
        Ok(units_at(d, shape(t).1)?.to_le_bytes().to_vec())
    }

    fn decode_record(&self, field: &[u8], t: &DataType) -> Option<Value> {
        let word = <[u8; NUMBER_BYTES]>::try_from(field).ok()?;
        Some(Value::Exact(Decimal::new(i128::from(i64::from_le_bytes(word)), shape(t).1)))
    }

    fn encode_key(&self, v: &Value, t: &DataType, out: &mut Vec<u8>) -> Result<()> {
        let Value::Exact(d) = v else { return Err(mismatch(v, t)) };
        let units = u64::from_be_bytes(units_at(d, shape(t).1)?.to_be_bytes());
        out.extend_from_slice(&(units ^ SIGN).to_be_bytes());
        Ok(())
    }

    fn decode_key(&self, rest: &mut &[u8], t: &DataType) -> Result<Value> {
        let word = <[u8; NUMBER_BYTES]>::try_from(take(rest, NUMBER_BYTES)?).map_err(|_| super::malformed())?;
        let units = i64::from_be_bytes((u64::from_be_bytes(word) ^ SIGN).to_be_bytes());
        Ok(Value::Exact(Decimal::new(i128::from(units), shape(t).1)))
    }

    /// Rounded down at the column's scale, within 64 bits.
    fn anchor(&self, v: &Value, t: &DataType) -> Option<(Value, bool)> {
        let Value::Exact(d) = v else { return None };
        let scale = shape(t).1;
        let units = floor_units(d, scale);
        Some((Value::Exact(Decimal::new(i128::from(units), scale)), units_at(d, scale).ok() == Some(units)))
    }
}
