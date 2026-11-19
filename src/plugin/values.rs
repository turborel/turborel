//! \[DESIGN\] Values in the plug-in protocol, version 1 (RULES.md D171):
//! a tag byte, then the value's body.
//!
//! ```text
//! 0  null                      (nothing)
//! 1  BOOLEAN                   u8 0 or 1
//! 2  exact numeric             units i128 | scale u32
//! 3  approximate numeric       the f64's bits u64
//! 4  character string, CLOB    str
//! 5  binary string, BLOB       bytes
//! 6  text form                 str: DATE, TIME, TIMESTAMP, INTERVAL, DECFLOAT,
//!                              JSON, XML and other types, cast to CHARACTER VARYING
//! 7  ARRAY, MULTISET           count u32 | each a value
//! 8  ROW                       count u32 | each a value
//! 9  VECTOR, dense             dims u32 | each f64
//! 10 VECTOR, sparse            dims u32 | count u32 | each index u32 and f64
//! ```
//!
//! What a program returns is decoded as it is written, text form as a
//! character string, and stored in the type its manifest declares
//! (`stored`), as a routine's result is assigned [ISO/IEC 9075-2:2023
//! §9.2]. A structured type's value and a reference carry identities of
//! the database, so no protocol 1 message carries one.

use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::value::lob::LobKind;
use crate::value::{Binary, DataType, Decimal, Value, assign, cast};
use crate::vector::{Element, Layout, Vector, value_of, vector_of};

use super::errors::violation;

/// The tag of the null value.
pub const NULL: u8 = 0;
/// The tag of a truth value.
pub const BOOLEAN: u8 = 1;
/// The tag of an exact number.
pub const EXACT: u8 = 2;
/// The tag of an approximate number.
pub const APPROXIMATE: u8 = 3;
/// The tag of a character string.
pub const CHARACTER: u8 = 4;
/// The tag of a binary string.
pub const BINARY: u8 = 5;
/// The tag of a value in its text form.
pub const TEXT: u8 = 6;
/// The tag of an array or multiset.
pub const ARRAY: u8 = 7;
/// The tag of a row.
pub const ROW: u8 = 8;
/// The tag of a dense vector.
pub const DENSE_VECTOR: u8 = 9;
/// The tag of a sparse vector.
pub const SPARSE_VECTOR: u8 = 10;

/// The longest text form, in characters: a value's cast to CHARACTER
/// VARYING of it.
const TEXT_LENGTH: u32 = 1_048_576;
/// What an assignment of a returned value cites.
const RETURNED: &str = "ISO/IEC 9075-2:2023 §9.2";

/// A count, as a u32.
fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Appends `v`.
///
/// # Errors
///
/// SQLSTATE 38T03 of a structured type's value or a reference, or of a
/// large object that cannot be read.
pub fn put_value(out: &mut Vec<u8>, v: &Value) -> Result<()> {
    if let Some(vector) = vector_of(v) {
        put_vector(out, &Vector::of(vector));
        return Ok(());
    }
    match v {
        Value::Null => out.put_u8(NULL),
        Value::Bool(b) => out.extend_from_slice(&[BOOLEAN, u8::from(*b)]),
        Value::Exact(d) => {
            out.put_u8(EXACT);
            out.put_i128(d.units);
            out.put_u32(d.scale);
        }
        Value::Approx(x) => {
            out.put_u8(APPROXIMATE);
            out.put_u64(x.to_bits());
        }
        Value::Char(s) => {
            out.put_u8(CHARACTER);
            out.put_str(s);
        }
        Value::Binary(b) => {
            out.put_u8(BINARY);
            out.put_bytes(b.bytes());
        }
        Value::Array(parts) | Value::Multiset(parts) => put_parts(out, ARRAY, parts)?,
        Value::Row(parts) => put_parts(out, ROW, parts)?,
        Value::Lob(_) => put_value(out, &read_whole(v)?)?,
        Value::Datetime(_) | Value::Interval(_) | Value::DecFloat(_) | Value::Json(_) | Value::Xml(_) | Value::Extension(_) => {
            out.put_u8(TEXT);
            out.put_str(&text_form(v));
        }
        Value::Structured(_) | Value::Reference(_) => {
            return Err(violation("a structured type's value or a reference crosses to no plug-in"));
        }
    }
    Ok(())
}

/// A large object's value whole, as a string of its kind.
fn read_whole(v: &Value) -> Result<Value> {
    let Value::Lob(l) = v else { return Ok(v.clone()) };
    let to = match l.kind() {
        LobKind::Character => DataType::varying(TEXT_LENGTH),
        LobKind::Binary => DataType::VarBinary { length: TEXT_LENGTH },
    };
    cast(v.clone(), &to).map_err(|e| violation(&format!("a large object could not be read: {}", e.msg)))
}

/// `v` cast to CHARACTER VARYING: its text form.
fn text_form(v: &Value) -> String {
    match cast(v.clone(), &DataType::varying(TEXT_LENGTH)) {
        Ok(Value::Char(s)) => s,
        Ok(_) | Err(_) => v.to_string(),
    }
}

/// An array's or row's parts, of tag `tag`.
fn put_parts(out: &mut Vec<u8>, tag: u8, parts: &[Value]) -> Result<()> {
    out.put_u8(tag);
    put_values(out, parts)
}

/// A vector, dense or sparse as it is laid out.
fn put_vector(out: &mut Vec<u8>, v: &Vector) {
    let values = v.values();
    match v.layout() {
        Layout::Dense => {
            out.put_u8(DENSE_VECTOR);
            out.put_u32(count(values.len()));
            for x in values {
                out.put_u64(x.to_bits());
            }
        }
        Layout::Sparse => {
            out.put_u8(SPARSE_VECTOR);
            out.put_u32(count(values.len()));
            let nonzero: Vec<(u32, f64)> = (0_u32..).zip(values).filter(|(_, x)| x.abs() > 0.0_f64).collect();
            out.put_u32(count(nonzero.len()));
            for (i, x) in nonzero {
                out.put_u32(i);
                out.put_u64(x.to_bits());
            }
        }
    }
}

/// Appends `all`, counted.
///
/// # Errors
///
/// As `put_value`.
pub fn put_values(out: &mut Vec<u8>, all: &[Value]) -> Result<()> {
    out.put_u32(count(all.len()));
    all.iter().try_for_each(|v| put_value(out, v))
}

/// The value `r` holds next, as it is written: text form a character
/// string.
///
/// # Errors
///
/// SQLSTATE 38T03 if it is no value of protocol 1.
pub fn value(r: &mut Reader<'_>) -> Result<Value> {
    let v = match r.u8()? {
        NULL => Value::Null,
        BOOLEAN => Value::Bool(r.u8()? != 0),
        EXACT => {
            let units = r.i128()?;
            Value::Exact(Decimal { units, scale: r.u32()? })
        }
        APPROXIMATE => Value::Approx(f64::from_bits(r.u64()?)),
        CHARACTER | TEXT => Value::Char(r.str()?),
        BINARY => Value::Binary(Binary::new(r.bytes()?.to_vec())),
        ARRAY => Value::Array(r.nested(values)?.into()),
        ROW => Value::Row(r.nested(values)?.into()),
        DENSE_VECTOR => {
            let all: Vec<f64> = (0..r.u32()?).map(|_| r.u64().map(f64::from_bits)).collect::<Result<_>>()?;
            vector_value(Vector::dense(Element::Float64, all))?
        }
        SPARSE_VECTOR => {
            let dims = r.u32()?;
            let pairs: Vec<(u32, f64)> = (0..r.u32()?).map(|_| Ok((r.u32()?, f64::from_bits(r.u64()?)))).collect::<Result<_>>()?;
            vector_value(Vector::sparse(Element::Float64, dims, pairs))?
        }
        other => return Err(violation(&format!("no value is of tag {other}"))),
    };
    Ok(v)
}

/// The value of a vector made, or why it could not be made.
fn vector_value(made: Result<Vector>) -> Result<Value> {
    let image = made.and_then(|v| v.encode()).map_err(|e| violation(&format!("a vector is invalid: {}", e.msg)))?;
    Ok(value_of(image))
}

/// The values `r` holds next, counted.
///
/// # Errors
///
/// As `value`.
pub fn values(r: &mut Reader<'_>) -> Result<Vec<Value>> {
    (0..r.u32()?).map(|_| value(r)).collect()
}

/// `v`, a program's, stored in the declared type `t`: a string or number
/// assigned [ISO/IEC 9075-2:2023 §9.2], so a longer string or a number out
/// of range is a data exception; a text form, a vector, an array or a row
/// cast to it [§6.13].
///
/// # Errors
///
/// The assignment's or the cast's.
pub fn stored(v: Value, t: &DataType) -> Result<Value> {
    let assigned = match &v {
        Value::Null => return Ok(Value::Null),
        Value::Char(_) => t.is_character(),
        Value::Array(_) | Value::Row(_) | Value::Multiset(_) | Value::Extension(_) => false,
        Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Binary(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Lob(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Json(_)
        | Value::Xml(_) => true,
    };
    if assigned { assign(v, t, RETURNED) } else { cast(v, t) }
}

#[cfg(test)]
mod tests;
