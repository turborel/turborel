//! [ISO/IEC 9075-2:2023 §6.1] BINARY(length), and BINARY VARYING(length)
//! or VARBINARY(length): binary strings of fixed and of variable length,
//! in octets. Their values are `Value::Binary`, compared octet by octet
//! [§8.2 GR5].
//!
//! A BINARY(length) value is its octets in a record, of the fixed width
//! they take; a BINARY VARYING value, its octets, of variable length.
//! Either's key is its octets' key (`value::binary`).

use std::cmp::Ordering;

use super::{Descriptor, SqlType};
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError, SqlState, fail_with};
use crate::value::cast::uncastable;
use crate::value::{Binary, Bucket, DataType, Value, assignment_rule, unassignable};

/// [ISO/IEC 9075-2:2023 §6.1] BINARY, the name of `<binary string type>`.
/// Not a key word here, as no key word of SQL-92, so a column of SQL-92
/// named BINARY keeps its name.
pub const BINARY_NAME: &str = "BINARY";
/// [ISO/IEC 9075-2:2023 §6.1 SR1] VARBINARY, BINARY VARYING by another
/// name; no key word here, as BINARY.
pub const VARBINARY_NAME: &str = "VARBINARY";

/// The kind of BINARY(length), or of BINARY VARYING(length).
#[derive(Debug)]
pub struct BinaryKind {
    varying: bool,
}

/// BINARY(length).
pub static BINARY: BinaryKind = BinaryKind { varying: false };
/// BINARY VARYING(length).
pub static BINARY_VARYING: BinaryKind = BinaryKind { varying: true };

/// The length, or maximum length, of `t`, a binary string type.
fn length_of(t: &DataType) -> u32 {
    match t {
        DataType::Binary { length } | DataType::VarBinary { length } => *length,
        DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::Boolean
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_)
        | DataType::Extension(_) => 0,
    }
}

/// The octets of `v`, if it is a binary string.
fn binary(v: &Value) -> Option<&Binary> {
    if let Value::Binary(b) = v { Some(b) } else { None }
}

/// A value not of a column of `t`'s type.
fn mismatch(v: &Value, t: &DataType) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] {v} is not a value of a column of type {t}"))
}

/// `length` as a count of octets in memory.
fn octets(length: u32) -> usize {
    usize::try_from(length).unwrap_or(usize::MAX)
}

/// `b` extended on the right by X'00' octets to `length`, if shorter.
fn padded(b: &Binary, length: u32) -> Binary {
    let mut bytes = b.bytes().to_vec();
    bytes.resize(octets(length).max(bytes.len()), 0);
    Binary::new(bytes)
}

impl BinaryKind {
    /// [ISO/IEC 9075-2:2023 §6.13 GR13] `b` as a value of `t`: its first
    /// octets, with the warning string data, right truncation if any are
    /// cut \[GR13a ii, GR13b ii\]; of a fixed length, extended with X'00'
    /// octets if shorter \[GR13a iii\].
    fn fitted(&self, b: &Binary, t: &DataType) -> (Value, Option<SqlState>) {
        let length = length_of(t);
        let warning = (b.len() > length).then_some(SqlState::STRING_TRUNCATED);
        let cut = b.substring(1, length);
        let fitted = if self.varying { cut } else { padded(&cut, length) };
        (Value::Binary(fitted), warning)
    }
}

impl SqlType for BinaryKind {
    fn name(&self) -> &'static str {
        if self.varying { "BINARY VARYING" } else { BINARY_NAME }
    }

    fn written(&self, t: &DataType) -> String {
        format!("{}({})", self.name(), length_of(t))
    }

    /// [ISO/IEC 9075-2:2023 §4.4.1] Binary strings are comparable with
    /// binary strings alone.
    fn comparable(&self, other: &DataType) -> bool {
        matches!(other, DataType::Binary { .. } | DataType::VarBinary { .. })
    }

    /// [ISO/IEC 9075-2:2023 §8.2 GR5] octet by octet.
    fn compare(&self, a: &Value, b: &Value) -> Option<Ordering> {
        Some(binary(a)?.cmp(binary(b)?))
    }

    /// Its key, which equal binary strings share.
    fn bucket(&self, v: &Value) -> Option<Bucket> {
        let mut key = Vec::new();
        binary(v)?.put_key(&mut key);
        Some(Bucket::Binary(key.into_boxed_slice()))
    }

    /// [ISO/IEC 9075-2:2023 §9.2 GR3b viii-xii] Its first octets, if those
    /// past the length are all X'00' \[GR3b ix 1, GR3b xii 1\]; of a fixed
    /// length, extended with X'00' octets if shorter \[GR3b x\].
    fn assign(&self, v: Value, t: &DataType, rule: &str) -> Result<Value> {
        let Some(b) = binary(&v) else { return unassignable(&v, t, rule) };
        let (len, length) = (b.len(), length_of(t));
        let past = b.bytes().get(octets(length)..).unwrap_or_default();
        if past.iter().any(|&o| o != 0) {
            let at = if self.varying { "GR3b xii 2" } else { "GR3b ix 2" };
            let msg = format!(
                "[{}] a binary string of {len} octets, not X'00' beyond {length}, does not fit {t}",
                assignment_rule(rule, &format!("ISO/IEC 9075-2:2023 §9.2 {at}"))
            );
            return fail_with(Code::Data, SqlState::STRING_DATA_RIGHT_TRUNCATION, msg);
        }
        if len == length || (self.varying && len < length) {
            return Ok(v);
        }
        let cut = b.substring(1, length);
        Ok(Value::Binary(if self.varying { cut } else { padded(&cut, length) }))
    }

    /// [ISO/IEC 9075-2:2023 §6.13 GR13] As `cast_warned`, its warning not
    /// raised.
    fn cast(&self, v: Value, t: &DataType) -> Result<Value> {
        self.cast_warned(v, t).map(|(cast, _)| cast)
    }

    /// [ISO/IEC 9075-2:2023 §6.13 SR7, GR13] A binary string fitted to the
    /// type, with its warning (`fitted`); no value of another type.
    fn cast_warned(&self, v: Value, t: &DataType) -> Result<(Value, Option<SqlState>)> {
        match &v {
            Value::Null => Ok((Value::Null, None)),
            Value::Binary(b) => Ok(self.fitted(b, t)),
            Value::Char(_)
            | Value::Exact(_)
            | Value::Approx(_)
            | Value::Bool(_)
            | Value::Datetime(_)
            | Value::Interval(_)
            | Value::Extension(_) => uncastable(&v, t).map(|cast| (cast, None)),
        }
    }

    /// [ISO/IEC 9075-2:2023 §5.3] `<binary string literal>`: X'...'.
    fn literal(&self, v: &Value) -> String {
        binary(v).map_or_else(|| v.to_string(), |b| format!("X'{b}'"))
    }

    /// Its octets as hexits, two to each.
    fn text(&self, v: &Value) -> String {
        v.to_string()
    }

    fn position(&self, _: &Value) -> Option<f64> {
        None
    }

    /// [ISO/IEC 9075-2:2023 §6.1 GR13] Its length in octets, and the
    /// octets it takes; no collation, as no character type.
    fn descriptor(&self, t: &DataType) -> Descriptor {
        let length = length_of(t);
        Descriptor { length: Some(length), octets: Some(octets(length)), ..Descriptor::of(self.name()) }
    }

    fn put_value(&self, v: &Value, out: &mut Vec<u8>) {
        out.put_bytes(binary(v).map(Binary::bytes).unwrap_or_default());
    }

    fn value(&self, r: &mut Reader<'_>) -> Result<Value> {
        Ok(Value::Binary(Binary::new(r.bytes()?.to_vec())))
    }

    fn put_params(&self, t: &DataType, out: &mut Vec<u8>) {
        out.put_u32(length_of(t));
    }

    fn params(&self, r: &mut Reader<'_>) -> Result<DataType> {
        let length = r.u32()?;
        Ok(if self.varying { DataType::VarBinary { length } } else { DataType::Binary { length } })
    }

    /// Of a fixed length, the octets it takes; of a variable, none.
    fn record_width(&self, t: &DataType) -> Option<usize> {
        (!self.varying).then(|| octets(length_of(t)))
    }

    /// Its octets, which of a fixed length are exactly the length.
    fn encode_record(&self, v: &Value, t: &DataType) -> Result<Vec<u8>> {
        let b = binary(v).ok_or_else(|| mismatch(v, t))?;
        if !self.varying && b.len() != length_of(t) {
            return Err(mismatch(v, t));
        }
        Ok(b.bytes().to_vec())
    }

    fn decode_record(&self, field: &[u8], t: &DataType) -> Option<Value> {
        let fits = self.varying || field.len() == octets(length_of(t));
        fits.then(|| Value::Binary(Binary::new(field.to_vec())))
    }

    fn encode_key(&self, v: &Value, t: &DataType, out: &mut Vec<u8>) -> Result<()> {
        binary(v).ok_or_else(|| mismatch(v, t))?.put_key(out);
        Ok(())
    }

    fn decode_key(&self, rest: &mut &[u8], _: &DataType) -> Result<Value> {
        Binary::take_key(rest).map(Value::Binary).ok_or_else(super::malformed)
    }

    /// Itself: a binary string of any length has a key.
    fn anchor(&self, v: &Value, _: &DataType) -> Option<(Value, bool)> {
        binary(v).map(|_| (v.clone(), true))
    }
}
