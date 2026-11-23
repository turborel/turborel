//! [X3.135-1986 §5.5] NUMERIC(precision, scale), and DECIMAL, INTEGER and
//! SMALLINT, which are exact numbers of it, as is SQL:2023's BIGINT
//! [ISO/IEC 9075-2:2023 §6.1 SR30].
//!
//! Its record bytes and its key are the units at the column's scale: a
//! 64-bit integer, which holds `NARROW_EXACT_PRECISION` (18) digits, of a
//! type of no more, and a 128-bit one, which holds `MAX_EXACT_PRECISION`
//! (38), of a wider type (RULES.md D698). Each is little-endian in a
//! record; in a key big-endian, its sign bit flipped, so the bytes order
//! as the numbers do.

use std::cmp::Ordering;

use super::{Descriptor, NUMBER_BYTES, SIGN, SqlType, take};
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError, SqlState, fail_with};
use crate::value::NARROW_EXACT_PRECISION;
use crate::value::cast::{approximate_to_exact, numeric_literal, uncastable};
use crate::value::comparison::{compare_numbers, number_bucket};
use crate::value::{Bucket, DataType, Decimal, Value, assignment_rule, pow10, unassignable};
use crate::value::{Spelled, spelling};

/// The kind of NUMERIC(precision, scale).
#[derive(Debug)]
pub struct Exact;

/// The one value of it.
pub static EXACT: Exact = Exact;

/// [ISO/IEC 9075-2:2023 §6.1] BIGINT, no key word of SQL-92, so read as
/// an identifier where a data type is.
pub const BIGINT_NAME: &str = "BIGINT";

/// [§21.3.5 Description 3] The radix of its precision: decimal digits.
const DECIMAL_RADIX: u32 = 10;

/// Bytes of the units of a wide type in a record or a key: a 128-bit word.
const WIDE_BYTES: usize = 16;
/// The sign bit of a 128-bit word.
const WIDE_SIGN: u128 = 1 << 127;

/// Whether `t`'s values are kept in 128 bits: of more digits than 64 bits
/// always hold.
fn wide(t: &DataType) -> bool {
    shape(t).0 > NARROW_EXACT_PRECISION
}

/// \[DESIGN\] (RULES.md D698) The scale of `t` if it is an exact numeric
/// type whose values 64 bits hold, as a record does and a batch's
/// integers (`storage::Ints`): none of a wider one, or of another type.
pub fn narrow_scale(t: &DataType) -> Option<u32> {
    let DataType::Exact { precision, scale, .. } = t else { return None };
    (*precision <= NARROW_EXACT_PRECISION).then_some(*scale)
}

/// \[DESIGN\] (RULES.md D956) The key of `v` among the keys of a column of
/// `t`, and so its anchor, exactly, if `t` is an exact numeric type whose
/// values 64 bits hold and `v` an exact number at its scale that 64 bits
/// hold: its units, sign bit flipped, big-endian, as `encode_key` writes
/// them. None of any other, whose anchor is made as `anchor` makes it.
#[inline]
pub(crate) fn narrow_key(v: &Value, t: &DataType) -> Option<[u8; NUMBER_BYTES]> {
    let (Value::Exact(d), Some(scale)) = (v, narrow_scale(t)) else { return None };
    let units = i64::try_from(d.units).ok().filter(|_| d.scale == scale)?;
    Some(narrow_word(units))
}

/// The key of `units` of a column whose values 64 bits hold.
#[inline]
fn narrow_word(units: i64) -> [u8; NUMBER_BYTES] {
    (u64::from_be_bytes(units.to_be_bytes()) ^ SIGN).to_be_bytes()
}

/// The precision and scale of `t`, an exact numeric type.
fn shape(t: &DataType) -> (u32, u32) {
    if let DataType::Exact { precision, scale, .. } = t { (*precision, *scale) } else { (0, 0) }
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
    wide_units_at(value, scale).and_then(|u| i64::try_from(u).map_err(|_| unstorable(value, scale, "64")))
}

/// The units of `value` at `scale`, if they are whole there: as a column
/// of that scale and a wide type stores them.
///
/// # Errors
///
/// SQLCODE -910 if `value` has more digits after the point than `scale`
/// keeps, or more digits in all than 128 bits hold.
fn wide_units_at(value: &Decimal, scale: u32) -> Result<i128> {
    // At the column's scale already, as a key's value mostly is, its
    // units are those, with no power of ten made nor multiplied by.
    if value.scale == scale {
        return Ok(value.units);
    }
    let units = if value.scale <= scale {
        pow10(scale.saturating_sub(value.scale)).and_then(|f| value.units.checked_mul(f))
    } else {
        pow10(value.scale.saturating_sub(scale)).filter(|&f| value.units.checked_rem(f) == Some(0)).and_then(|f| value.units.checked_div(f))
    };
    units.ok_or_else(|| unstorable(value, scale, "128"))
}

/// A value a column of `scale` cannot store in `bits` bits.
fn unstorable(value: &Decimal, scale: u32, bits: &str) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] {value} cannot be stored at scale {scale} in {bits} bits: it is not of the column's type"))
}

/// The units of the greatest number at `scale` not above `d`, within 128
/// bits.
fn floor_units(d: &Decimal, scale: u32) -> i128 {
    let clamp = |negative: bool| if negative { i128::MIN } else { i128::MAX };
    if d.scale == scale {
        return d.units;
    }
    if d.scale <= scale {
        return pow10(scale.saturating_sub(d.scale)).and_then(|f| d.units.checked_mul(f)).unwrap_or_else(|| clamp(d.units < 0));
    }
    // Divided by more than 10^38, a value of 38 digits is below 1.
    pow10(d.scale.saturating_sub(scale)).map_or(if d.units < 0 { -1 } else { 0 }, |f| d.units.div_euclid(f))
}

/// `units` within 64 bits, clamped.
fn narrowed(units: i128) -> i64 {
    i64::try_from(units).unwrap_or(if units < 0 { i64::MIN } else { i64::MAX })
}

impl SqlType for Exact {
    fn name(&self) -> &'static str {
        "NUMERIC"
    }

    /// As written: INTEGER, SMALLINT, BIGINT, DECIMAL(p,s), else
    /// NUMERIC(p,s).
    fn written(&self, t: &DataType) -> String {
        let (precision, scale) = shape(t);
        match spelling(t) {
            Spelled::Integer => "INTEGER".to_owned(),
            Spelled::Smallint => "SMALLINT".to_owned(),
            Spelled::Bigint => BIGINT_NAME.to_owned(),
            Spelled::Decimal => format!("DECIMAL({precision},{scale})"),
            Spelled::Numeric | Spelled::Float | Spelled::Real | Spelled::DoublePrecision => format!("NUMERIC({precision},{scale})"),
        }
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
                        format!("[{}] {d} does not fit {t} without losing leading digits", assignment_rule(rule, "§9.2 GR3k")),
                    ),
                }
            }
            Value::Approx(x) => approximate_to_exact(x, (precision, scale), rule),
            // [ISO/IEC 9075-2:2023 §9.2 GR3b xv] its digits past the scale
            // truncated, as an exact number's (IA002).
            Value::DecFloat(d) => match d.to_decimal(scale).filter(|out| out.fits(precision, scale)) {
                Some(out) => Ok(Value::Exact(out)),
                None => fail_with(
                    Code::Data,
                    SqlState::NUMERIC_VALUE_OUT_OF_RANGE,
                    format!(
                        "[{}] {d} does not fit {t} without losing leading digits",
                        assignment_rule(rule, "ISO/IEC 9075-2:2023 §9.2 GR3b xv 3")
                    ),
                ),
            },
            Value::Null
            | Value::Char(_)
            | Value::Binary(_)
            | Value::Lob(_)
            | Value::Bool(_)
            | Value::Datetime(_)
            | Value::Interval(_)
            | Value::Array(_)
            | Value::Multiset(_)
            | Value::Structured(_)
            | Value::Reference(_)
            | Value::Row(_)
            | Value::Json(_)
            | Value::Extension(_)
            | Value::Xml(_) => unassignable(&v, t, rule),
        }
    }

    /// [ISO/IEC 9075:1992 §6.10 GR3] A string's `<signed numeric literal>`,
    /// its spaces trimmed, cast \[GR3b\]; a number's extra digits truncated,
    /// as store assignment does, an approximate one rounded \[GR3a,
    /// implementation-defined\]; an interval of one field, as a number of
    /// it \[GR3c\].
    fn cast(&self, v: Value, t: &DataType) -> Result<Value> {
        match v {
            Value::Null => Ok(Value::Null),
            Value::Char(s) => self.cast(numeric_literal(&s)?, t),
            Value::Exact(_) => self.assign(v, t, "ISO/IEC 9075:1992 §6.10 GR3a"),
            Value::Approx(x) => approximate_to_exact(x, shape(t), "ISO/IEC 9075:1992 §6.10 GR3a ii"),
            Value::DecFloat(_) => self.assign(v, t, "ISO/IEC 9075-2:2023 §6.13 GR8a"),
            // [GR3c] an interval of one field, as a number of it.
            Value::Interval(i) => self.assign(Value::Exact(super::interval::as_number(&i)), t, "ISO/IEC 9075:1992 §6.10 GR3c"),
            Value::Binary(_)
            | Value::Lob(_)
            | Value::Bool(_)
            | Value::Datetime(_)
            | Value::Array(_)
            | Value::Multiset(_)
            | Value::Structured(_)
            | Value::Reference(_)
            | Value::Row(_)
            | Value::Json(_)
            | Value::Extension(_)
            | Value::Xml(_) => uncastable(&v, t),
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
    /// [ISO/IEC 9075:1992 §21.3.5] Of the name it was written with:
    /// INTEGER, SMALLINT, DECIMAL or NUMERIC, or BIGINT [ISO/IEC
    /// 9075-2:2023 §6.1].
    fn descriptor(&self, t: &DataType) -> Descriptor {
        let (precision, scale) = shape(t);
        let name = match spelling(t) {
            Spelled::Integer => "INTEGER",
            Spelled::Smallint => "SMALLINT",
            Spelled::Bigint => BIGINT_NAME,
            Spelled::Decimal => "DECIMAL",
            Spelled::Numeric | Spelled::Float | Spelled::Real | Spelled::DoublePrecision => self.name(),
        };
        Descriptor { precision: Some(precision), radix: Some(DECIMAL_RADIX), scale: Some(scale), ..Descriptor::of(name) }
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
        Ok(DataType::Exact { precision: r.u32()?, scale: r.u32()?, spelled: Spelled::Numeric })
    }

    fn record_width(&self, t: &DataType) -> Option<usize> {
        Some(if wide(t) { WIDE_BYTES } else { NUMBER_BYTES })
    }

    fn encode_record(&self, v: &Value, t: &DataType) -> Result<Vec<u8>> {
        let Value::Exact(d) = v else { return Err(mismatch(v, t)) };
        let scale = shape(t).1;
        if wide(t) {
            return Ok(wide_units_at(d, scale)?.to_le_bytes().to_vec());
        }
        Ok(units_at(d, scale)?.to_le_bytes().to_vec())
    }

    fn encode_record_into(&self, v: &Value, t: &DataType, out: &mut Vec<u8>) -> Result<()> {
        let Value::Exact(d) = v else { return Err(mismatch(v, t)) };
        let scale = shape(t).1;
        if wide(t) {
            out.extend_from_slice(&wide_units_at(d, scale)?.to_le_bytes());
        } else {
            out.extend_from_slice(&units_at(d, scale)?.to_le_bytes());
        }
        Ok(())
    }

    /// Of its width, which its type gives: 8 bytes or 16.
    fn decode_record(&self, field: &[u8], t: &DataType) -> Option<Value> {
        let units = match <[u8; NUMBER_BYTES]>::try_from(field) {
            Ok(word) => i128::from(i64::from_le_bytes(word)),
            Err(_) => i128::from_le_bytes(<[u8; WIDE_BYTES]>::try_from(field).ok()?),
        };
        Some(Value::Exact(Decimal::new(units, shape(t).1)))
    }

    fn encode_key(&self, v: &Value, t: &DataType, out: &mut Vec<u8>) -> Result<()> {
        let Value::Exact(d) = v else { return Err(mismatch(v, t)) };
        let scale = shape(t).1;
        if wide(t) {
            let units = u128::from_be_bytes(wide_units_at(d, scale)?.to_be_bytes());
            out.extend_from_slice(&(units ^ WIDE_SIGN).to_be_bytes());
            return Ok(());
        }
        out.extend_from_slice(&narrow_word(units_at(d, scale)?));
        Ok(())
    }

    fn decode_key(&self, rest: &mut &[u8], t: &DataType) -> Result<Value> {
        let scale = shape(t).1;
        if wide(t) {
            let word = <[u8; WIDE_BYTES]>::try_from(take(rest, WIDE_BYTES)?).map_err(|_| super::malformed())?;
            let units = i128::from_be_bytes((u128::from_be_bytes(word) ^ WIDE_SIGN).to_be_bytes());
            return Ok(Value::Exact(Decimal::new(units, scale)));
        }
        let word = <[u8; NUMBER_BYTES]>::try_from(take(rest, NUMBER_BYTES)?).map_err(|_| super::malformed())?;
        let units = i64::from_be_bytes((u64::from_be_bytes(word) ^ SIGN).to_be_bytes());
        Ok(Value::Exact(Decimal::new(i128::from(units), scale)))
    }

    /// Rounded down at the column's scale, within the bits it is kept in.
    fn anchor(&self, v: &Value, t: &DataType) -> Option<(Value, bool)> {
        let Value::Exact(d) = v else { return None };
        let scale = shape(t).1;
        let floor = floor_units(d, scale);
        let units = if wide(t) { floor } else { i128::from(narrowed(floor)) };
        let exact = wide_units_at(d, scale).ok() == Some(units);
        Some((Value::Exact(Decimal::new(units, scale)), exact))
    }
}

// Unit tests: records and keys of narrow and wide exact types (RULES.md
// R-TEST-1, D698).
#[cfg(test)]
mod tests;
