//! [ISO/IEC 9075:1992 §6.1] BIT(length) and BIT VARYING(length): bit
//! strings of fixed and of variable length. Their values are
//! `Value::Bits`, compared bit by bit [§8.2 GR4].
//!
//! A BIT(length) value is its bytes in a record, of the fixed width they
//! take; a BIT VARYING value, of variable length, the count of the unused
//! bits of its last byte, then its bytes. Either's key is its bits' key
//! (`value::bits`).

use std::cmp::Ordering;

use super::{Descriptor, SqlType};
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError, SqlState, fail_with};
use crate::value::cast::uncastable;
use crate::value::{BYTE_BITS, Bits, Bucket, DataType, Value, unassignable};

/// The kind of BIT(length), or of BIT VARYING(length).
#[derive(Debug)]
pub struct Bit {
    varying: bool,
}

/// BIT(length).
pub static BIT: Bit = Bit { varying: false };
/// BIT VARYING(length).
pub static BIT_VARYING: Bit = Bit { varying: true };

/// The length, or maximum length, of `t`, a bit string type.
fn length_of(t: &DataType) -> u32 {
    match t {
        DataType::Bit { length } | DataType::BitVarying { length } => *length,
        DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::Extension(_) => 0,
    }
}

/// The bits of `v`, if it is a bit string.
fn bits(v: &Value) -> Option<&Bits> {
    if let Value::Bits(b) = v { Some(b) } else { None }
}

/// A value not of a column of `t`'s type.
fn mismatch(v: &Value, t: &DataType) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] {v} is not a value of a column of type {t}"))
}

/// Bytes that hold `bits` bits.
fn bytes_for(bits: u32) -> usize {
    usize::try_from(bits.div_ceil(BYTE_BITS)).unwrap_or(usize::MAX)
}

/// The bits of `s`'s form-of-use, UTF-8, as CAST takes a character string
/// to a bit string [§6.10 GR7, GR8].
fn of_text(s: &str) -> Bits {
    let len = u32::try_from(s.len()).unwrap_or(u32::MAX).saturating_mul(BYTE_BITS);
    Bits::new(s.as_bytes().to_vec(), len).unwrap_or_default()
}

impl Bit {
    /// [§6.10 GR7, GR8] `b` as a value of `t`: of a fixed length, its
    /// first bits, or it extended with 0 bits; of a variable length, its
    /// first bits at most. The warnings of truncating and of padding are
    /// not raised.
    fn fitted(&self, b: &Bits, t: &DataType) -> Value {
        let length = length_of(t);
        let cut = b.substring(1, length);
        if self.varying || cut.len() == length {
            return Value::Bits(cut);
        }
        let mut bytes = cut.bytes().to_vec();
        bytes.resize(bytes_for(length), 0);
        Value::Bits(Bits::new(bytes, length).unwrap_or_default())
    }
}

impl SqlType for Bit {
    fn name(&self) -> &'static str {
        if self.varying { "BIT VARYING" } else { "BIT" }
    }

    fn written(&self, t: &DataType) -> String {
        format!("{}({})", self.name(), length_of(t))
    }

    /// Bit strings are comparable with bit strings alone.
    fn comparable(&self, other: &DataType) -> bool {
        matches!(other, DataType::Bit { .. } | DataType::BitVarying { .. })
    }

    /// [ISO/IEC 9075:1992 §8.2 GR4] bit by bit.
    fn compare(&self, a: &Value, b: &Value) -> Option<Ordering> {
        Some(bits(a)?.cmp(bits(b)?))
    }

    /// Its key, which equal bit strings share.
    fn bucket(&self, v: &Value) -> Option<Bucket> {
        let mut key = Vec::new();
        bits(v)?.put_key(&mut key);
        Some(Bucket::Bits(key.into_boxed_slice()))
    }

    /// [ISO/IEC 9075:1992 §9.2 GR3f-GR3j] To a fixed length, a value of
    /// that length; to a variable length, one no longer than its maximum.
    fn assign(&self, v: Value, t: &DataType, rule: &str) -> Result<Value> {
        let Some(b) = bits(&v) else { return unassignable(&v, t, rule) };
        let (len, length) = (b.len(), length_of(t));
        if len > length {
            let at = if self.varying { "GR3j" } else { "GR3g" };
            let msg = format!("[{rule}; ISO/IEC 9075:1992 §9.2 {at}] a bit string of {len} bits does not fit {t}");
            return fail_with(Code::Data, SqlState::STRING_DATA_RIGHT_TRUNCATION, msg);
        }
        if len < length && !self.varying {
            let msg = format!("[{rule}; ISO/IEC 9075:1992 §9.2 GR3h] a bit string of {len} bits is not of {t}'s length");
            return fail_with(Code::Data, SqlState::STRING_DATA_LENGTH_MISMATCH, msg);
        }
        Ok(v)
    }

    /// [ISO/IEC 9075:1992 §6.10 GR7, GR8] A bit string, or a character
    /// string's bits, fitted to the type; no number \[SR4\].
    fn cast(&self, v: Value, t: &DataType) -> Result<Value> {
        match &v {
            Value::Null => Ok(Value::Null),
            Value::Bits(b) => Ok(self.fitted(b, t)),
            Value::Char(s) => Ok(self.fitted(&of_text(s), t)),
            Value::Exact(_) | Value::Approx(_) | Value::Extension(_) => uncastable(&v, t),
        }
    }

    /// [ISO/IEC 9075:1992 §5.3] `<bit string literal>`: B'...'.
    fn literal(&self, v: &Value) -> String {
        bits(v).map_or_else(|| v.to_string(), |b| format!("B'{b}'"))
    }

    /// Its bits as 0s and 1s.
    fn text(&self, v: &Value) -> String {
        v.to_string()
    }

    fn position(&self, _: &Value) -> Option<f64> {
        None
    }

    /// [ISO/IEC 9075:1992 §21.3.5 Description] Its length in bits, and the
    /// octets they take; no collation, as no character type.
    fn descriptor(&self, t: &DataType) -> Descriptor {
        let length = length_of(t);
        Descriptor { length: Some(length), octets: Some(bytes_for(length)), ..Descriptor::of(self.name()) }
    }

    fn put_value(&self, v: &Value, out: &mut Vec<u8>) {
        let b = bits(v).cloned().unwrap_or_default();
        out.put_u32(b.len());
        out.put_bytes(b.bytes());
    }

    fn value(&self, r: &mut Reader<'_>) -> Result<Value> {
        let len = r.u32()?;
        let bytes = r.bytes()?;
        Bits::new(bytes.to_vec(), len).map(Value::Bits).map_err(|_| r.corrupt("a bit string is shorter than its length"))
    }

    fn put_params(&self, t: &DataType, out: &mut Vec<u8>) {
        out.put_u32(length_of(t));
    }

    fn params(&self, r: &mut Reader<'_>) -> Result<DataType> {
        let length = r.u32()?;
        Ok(if self.varying { DataType::BitVarying { length } } else { DataType::Bit { length } })
    }

    /// Of a fixed length, the bytes it takes; of a variable, none.
    fn record_width(&self, t: &DataType) -> Option<usize> {
        (!self.varying).then(|| bytes_for(length_of(t)))
    }

    fn encode_record(&self, v: &Value, t: &DataType) -> Result<Vec<u8>> {
        let b = bits(v).ok_or_else(|| mismatch(v, t))?;
        if !self.varying {
            if b.len() != length_of(t) {
                return Err(mismatch(v, t));
            }
            return Ok(b.bytes().to_vec());
        }
        // The unused bits of its last byte, then its bytes.
        let total = u32::try_from(b.bytes().len()).unwrap_or(u32::MAX).saturating_mul(BYTE_BITS);
        let unused = u8::try_from(total.saturating_sub(b.len())).unwrap_or(0);
        Ok([&[unused][..], b.bytes()].concat())
    }

    fn decode_record(&self, field: &[u8], t: &DataType) -> Option<Value> {
        if !self.varying {
            return Bits::new(field.to_vec(), length_of(t)).ok().map(Value::Bits);
        }
        let (&unused, bytes) = field.split_first()?;
        let total = u32::try_from(bytes.len()).ok()?.checked_mul(BYTE_BITS)?;
        Bits::new(bytes.to_vec(), total.checked_sub(u32::from(unused))?).ok().map(Value::Bits)
    }

    fn encode_key(&self, v: &Value, t: &DataType, out: &mut Vec<u8>) -> Result<()> {
        bits(v).ok_or_else(|| mismatch(v, t))?.put_key(out);
        Ok(())
    }

    fn decode_key(&self, rest: &mut &[u8], _: &DataType) -> Result<Value> {
        Bits::take_key(rest).map(Value::Bits).ok_or_else(super::malformed)
    }

    /// Itself: a bit string of any length has a key.
    fn anchor(&self, v: &Value, _: &DataType) -> Option<(Value, bool)> {
        bits(v).map(|_| (v.clone(), true))
    }
}
