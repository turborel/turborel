//! A data type beyond the standard for the tests of plug-in types (RULES.md
//! D53): IDENT, a 16-byte identifier written as 32 hexadecimal digits, as
//! a UUID is without its dashes. The core's integration tests include this
//! file too (`#[path]`), so it is written once (RULES.md R-TEST-10), and
//! uses nothing but turborel's public API.

use std::cmp::Ordering;
use std::sync::Arc;

use std::fmt::Write;
use turborel::bytes::{Put, Reader};
use turborel::error::{Code, Result, SqlError};

use turborel::value::types::{Descriptor, ExtensionValue, TypeParams};
use turborel::value::{Bucket, DataType, SqlType, Value};

/// The bytes of an identifier.
pub const IDENT_BYTES: usize = 16;

/// The kind of IDENT.
#[derive(Debug)]
pub struct Ident;

/// The bytes of `v`, if it is an identifier.
fn bytes(v: &Value) -> Option<[u8; IDENT_BYTES]> {
    let Value::Extension(e) = v else { return None };
    (e.kind.name() == "IDENT").then(|| <[u8; IDENT_BYTES]>::try_from(e.bytes.as_ref()).ok()).flatten()
}

/// The identifier of `b`.
pub fn ident(b: [u8; IDENT_BYTES]) -> Value {
    Value::Extension(ExtensionValue { kind: &IDENT, bytes: Arc::from(&b[..]) })
}

/// The one value of the kind, as registered.
pub static IDENT: Ident = Ident;

/// It, as a type of it holds it.
pub static IDENT_KIND: &dyn SqlType = &IDENT;

/// The identifier `text` writes: 32 hexadecimal digits.
fn parse(text: &str) -> Option<[u8; IDENT_BYTES]> {
    let chars: Vec<char> = text.trim_end_matches(' ').chars().collect();
    let pairs = chars.chunks(2).map(|p| u8::from_str_radix(&p.iter().collect::<String>(), 16).ok()).collect::<Option<Vec<u8>>>()?;
    <[u8; IDENT_BYTES]>::try_from(pairs.as_slice()).ok().filter(|_| chars.len() == IDENT_BYTES.saturating_mul(2))
}

fn mismatch(v: &Value) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] {v} is not an IDENT"))
}

impl SqlType for Ident {
    fn name(&self) -> &'static str {
        "IDENT"
    }

    fn written(&self, _: &DataType) -> String {
        "IDENT".to_owned()
    }

    fn comparable(&self, other: &DataType) -> bool {
        if let DataType::Extension(e) = other { e.kind.name() == "IDENT" } else { false }
    }

    fn compare(&self, a: &Value, b: &Value) -> Option<Ordering> {
        Some(bytes(a)?.cmp(&bytes(b)?))
    }

    fn bucket(&self, v: &Value) -> Option<Bucket> {
        Some(Bucket::Extension(Box::from(bytes(v)?.as_slice())))
    }

    fn assign(&self, v: Value, _: &DataType, rule: &str) -> Result<Value> {
        if bytes(&v).is_some() {
            return Ok(v);
        }
        Err(SqlError::new(Code::Data, format!("[{rule}] {v} is not an IDENT")))
    }

    fn cast(&self, v: Value, _: &DataType) -> Result<Value> {
        match &v {
            Value::Null => Ok(Value::Null),
            Value::Char(s) => {
                parse(s).map(ident).ok_or_else(|| SqlError::new(Code::Data, format!("[DESIGN] '{s}' is not 32 hexadecimal digits")))
            }
            Value::Extension(_) => bytes(&v).map(ident).ok_or_else(|| mismatch(&v)),
            Value::Exact(_) | Value::Approx(_) | Value::Bits(_) | Value::Datetime(_) => Err(mismatch(&v)),
        }
    }

    fn literal(&self, v: &Value) -> String {
        format!("CAST('{}' AS IDENT)", self.text(v))
    }

    fn text(&self, v: &Value) -> String {
        bytes(v).map_or_else(String::new, |b| {
            b.iter().fold(String::new(), |mut out, x| {
                let _ = write!(out, "{x:02X}");
                out
            })
        })
    }

    fn position(&self, _: &Value) -> Option<f64> {
        None
    }

    fn descriptor(&self, _: &DataType) -> Descriptor {
        Descriptor::of("IDENT")
    }

    fn put_value(&self, v: &Value, out: &mut Vec<u8>) {
        out.put_bytes(&bytes(v).unwrap_or_default());
    }

    fn value(&self, r: &mut Reader<'_>) -> Result<Value> {
        let read = r.bytes()?;
        <[u8; IDENT_BYTES]>::try_from(read).map(ident).map_err(|_| r.corrupt("an IDENT is not 16 bytes"))
    }

    fn put_params(&self, _: &DataType, _: &mut Vec<u8>) {}

    fn params(&self, _: &mut Reader<'_>) -> Result<DataType> {
        Ok(DataType::Extension(turborel::value::types::ExtensionType { kind: &IDENT_KIND, params: TypeParams::default() }))
    }

    fn record_width(&self, _: &DataType) -> Option<usize> {
        Some(IDENT_BYTES)
    }

    fn encode_record(&self, v: &Value, _: &DataType) -> Result<Vec<u8>> {
        bytes(v).map(|b| b.to_vec()).ok_or_else(|| mismatch(v))
    }

    fn decode_record(&self, field: &[u8], _: &DataType) -> Option<Value> {
        <[u8; IDENT_BYTES]>::try_from(field).ok().map(ident)
    }

    fn encode_key(&self, v: &Value, _: &DataType, out: &mut Vec<u8>) -> Result<()> {
        out.extend_from_slice(&bytes(v).ok_or_else(|| mismatch(v))?);
        Ok(())
    }

    fn decode_key(&self, rest: &mut &[u8], _: &DataType) -> Result<Value> {
        let (key, after) =
            rest.split_at_checked(IDENT_BYTES).ok_or_else(|| SqlError::new(Code::Corrupt, "[DESIGN] an IDENT key is short"))?;
        *rest = after;
        <[u8; IDENT_BYTES]>::try_from(key).map(ident).map_err(|_| SqlError::new(Code::Corrupt, "[DESIGN] an IDENT key is short"))
    }

    fn anchor(&self, v: &Value, _: &DataType) -> Option<(Value, bool)> {
        bytes(v).map(|_| (v.clone(), true))
    }
}

/// The type IDENT, of the kind registered with a database: the one
/// `Database::register_type(Box::new(Ident))` gives, as its tests write it.
pub fn ident_type() -> DataType {
    DataType::Extension(turborel::value::types::ExtensionType { kind: &IDENT_KIND, params: TypeParams::default() })
}
