//! [X3.135-1986 §5.5] FLOAT(precision), REAL and DOUBLE PRECISION: IEEE
//! binary floating point, single for a precision up to REAL's, else double.
//!
//! Its record bytes are its IEEE-754 bits, little-endian; its key the bits,
//! all inverted if negative, else the sign bit set, big-endian, so the bytes
//! order as the numbers do, -0.0 as 0.0, which SQL compares equal.

use std::cmp::Ordering;

use super::{Descriptor, NUMBER_BYTES, SIGN, SqlType, take};
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError};
use crate::value::cast::{at_precision, numeric_literal, uncastable};
use crate::value::comparison::{compare_numbers, number_bucket};
use crate::value::{Bucket, DOUBLE_PRECISION, DataType, REAL_PRECISION, Value, unassignable};

/// The kind of FLOAT(precision).
#[derive(Debug)]
pub struct Approximate;

/// The one value of it.
pub static APPROXIMATE: Approximate = Approximate;

/// [§21.3.5 Description 3] The radix of its precision: binary digits.
const BINARY_RADIX: u32 = 2;

/// The precision of `t`, an approximate numeric type.
fn precision_of(t: &DataType) -> u32 {
    if let DataType::Approximate { precision } = t { *precision } else { DOUBLE_PRECISION }
}

/// A value not of a column of `t`'s type.
fn mismatch(v: &Value, t: &DataType) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] {v} is not a value of a column of type {t}"))
}

impl SqlType for Approximate {
    fn name(&self) -> &'static str {
        "FLOAT"
    }

    fn written(&self, t: &DataType) -> String {
        format!("FLOAT({})", precision_of(t))
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

    /// [ISO/IEC 9075:1992 §9.2 GR3k] A number approximated in the type: a
    /// REAL's rounded to single precision [X3.135-1986 §8.7 GR7d].
    fn assign(&self, v: Value, t: &DataType, rule: &str) -> Result<Value> {
        match v.as_f64() {
            Some(x) => Ok(Value::Approx(at_precision(x, precision_of(t)))),
            None => unassignable(&v, t, rule),
        }
    }

    /// [ISO/IEC 9075:1992 §6.10 GR4] A string's `<signed numeric literal>`,
    /// its spaces trimmed, cast \[GR4b\]; a number approximated \[GR4a\].
    fn cast(&self, v: Value, t: &DataType) -> Result<Value> {
        match v {
            Value::Null => Ok(Value::Null),
            Value::Char(s) => self.cast(numeric_literal(&s)?, t),
            Value::Exact(d) => Ok(Value::Approx(at_precision(d.to_f64(), precision_of(t)))),
            Value::Approx(x) => Ok(Value::Approx(at_precision(x, precision_of(t)))),
            Value::Bits(_) | Value::Datetime(_) | Value::Interval(_) | Value::Extension(_) => uncastable(&v, t),
        }
    }

    fn literal(&self, v: &Value) -> String {
        v.to_string()
    }

    fn text(&self, v: &Value) -> String {
        v.to_string()
    }

    fn position(&self, v: &Value) -> Option<f64> {
        if let Value::Approx(f) = v { Some(*f) } else { None }
    }

    /// REAL or DOUBLE PRECISION, as it holds values, of binary precision.
    fn descriptor(&self, t: &DataType) -> Descriptor {
        let (name, bits) =
            if precision_of(t) <= REAL_PRECISION { ("REAL", REAL_PRECISION) } else { ("DOUBLE PRECISION", DOUBLE_PRECISION) };
        Descriptor { precision: Some(bits), radix: Some(BINARY_RADIX), ..Descriptor::of(name) }
    }

    /// The bits, so every value, NaN and -0.0 included, reads back as it
    /// was.
    fn put_value(&self, v: &Value, out: &mut Vec<u8>) {
        out.put_u64(if let Value::Approx(f) = v { f.to_bits() } else { 0 });
    }

    fn value(&self, r: &mut Reader<'_>) -> Result<Value> {
        Ok(Value::Approx(f64::from_bits(r.u64()?)))
    }

    fn put_params(&self, t: &DataType, out: &mut Vec<u8>) {
        out.put_u32(precision_of(t));
    }

    fn params(&self, r: &mut Reader<'_>) -> Result<DataType> {
        Ok(DataType::Approximate { precision: r.u32()? })
    }

    fn record_width(&self, _: &DataType) -> Option<usize> {
        Some(NUMBER_BYTES)
    }

    fn encode_record(&self, v: &Value, t: &DataType) -> Result<Vec<u8>> {
        let Value::Approx(f) = v else { return Err(mismatch(v, t)) };
        Ok(f.to_bits().to_le_bytes().to_vec())
    }

    fn decode_record(&self, field: &[u8], _: &DataType) -> Option<Value> {
        Some(Value::Approx(f64::from_bits(u64::from_le_bytes(<[u8; NUMBER_BYTES]>::try_from(field).ok()?))))
    }

    fn encode_key(&self, v: &Value, t: &DataType, out: &mut Vec<u8>) -> Result<()> {
        let Value::Approx(f) = v else { return Err(mismatch(v, t)) };
        // SQL compares -0.0 and 0.0 as equal, so they share a key.
        let bits = if *f == 0.0_f64 { 0 } else { f.to_bits() };
        let ordered = if bits & SIGN == 0 { bits | SIGN } else { !bits };
        out.extend_from_slice(&ordered.to_be_bytes());
        Ok(())
    }

    fn decode_key(&self, rest: &mut &[u8], _: &DataType) -> Result<Value> {
        let word = <[u8; NUMBER_BYTES]>::try_from(take(rest, NUMBER_BYTES)?).map_err(|_| super::malformed())?;
        let ordered = u64::from_be_bytes(word);
        Ok(Value::Approx(f64::from_bits(if ordered & SIGN == 0 { !ordered } else { ordered ^ SIGN })))
    }

    /// Itself: every approximate value is one of the column's.
    fn anchor(&self, v: &Value, _: &DataType) -> Option<(Value, bool)> {
        if let Value::Approx(_) = v { Some((v.clone(), true)) } else { None }
    }
}
