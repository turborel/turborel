//! \[DESIGN\] A value of a type beyond the standard, received where its
//! plug-in is not registered (RULES.md D53): a client's, from the wire. It
//! keeps the type's name, the value as text, and its kind's bytes, so it is
//! shown as its text and sent back as it came; nothing else is done with
//! it.
//!
//! ```text
//! bytes: the type's name | the value as text | its kind's bytes, each a
//!        string or bytes as `bytes::Put` writes them
//! ```

use std::cmp::Ordering;
use std::sync::Arc;

use super::{Descriptor, ExtensionValue, SqlType};
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError};
use crate::value::cast::uncastable;
use crate::value::{Bucket, DataType, Value, unassignable};

/// The kind of a value of a type not registered here.
#[derive(Debug)]
pub struct Unregistered;

/// The one value of it.
pub static UNREGISTERED: Unregistered = Unregistered;

/// It, as a type of it holds it (`types::Kind`).
pub static UNREGISTERED_KIND: &dyn SqlType = &UNREGISTERED;

/// A value of `name`'s type, not registered here: shown as `text`, its
/// kind's bytes `bytes`.
pub fn unregistered(name: &str, text: &str, bytes: &[u8]) -> Value {
    let mut held = Vec::new();
    held.put_str(name);
    held.put_str(text);
    held.put_bytes(bytes);
    Value::Extension(ExtensionValue { kind: &UNREGISTERED, bytes: Arc::from(held) })
}

/// The type's name, the text and the bytes of `v`, a value of a type not
/// registered here; none if it is not one.
pub fn parts(v: &Value) -> Option<(String, String, Vec<u8>)> {
    let Value::Extension(e) = v else { return None };
    let mut r = Reader::new(&e.bytes, "an unregistered value");
    Some((r.str().ok()?, r.str().ok()?, r.bytes().ok()?.to_vec()))
}

/// What is not done with a value of a type not registered here.
fn not_registered(what: &str) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] a value of a type not registered here is not {what}"))
}

impl SqlType for Unregistered {
    fn name(&self) -> &'static str {
        "UNREGISTERED"
    }

    fn written(&self, _: &DataType) -> String {
        self.name().to_owned()
    }

    fn comparable(&self, _: &DataType) -> bool {
        false
    }

    fn compare(&self, _: &Value, _: &Value) -> Option<Ordering> {
        None
    }

    fn bucket(&self, _: &Value) -> Option<Bucket> {
        None
    }

    fn assign(&self, v: Value, t: &DataType, rule: &str) -> Result<Value> {
        unassignable(&v, t, rule)
    }

    fn cast(&self, v: Value, t: &DataType) -> Result<Value> {
        uncastable(&v, t)
    }

    fn literal(&self, v: &Value) -> String {
        self.text(v)
    }

    fn text(&self, v: &Value) -> String {
        parts(v).map(|(_, text, _)| text).unwrap_or_default()
    }

    fn position(&self, _: &Value) -> Option<f64> {
        None
    }

    fn descriptor(&self, _: &DataType) -> Descriptor {
        Descriptor::of(self.name())
    }

    fn put_value(&self, v: &Value, out: &mut Vec<u8>) {
        if let Value::Extension(e) = v {
            out.put_bytes(&e.bytes);
        }
    }

    fn value(&self, r: &mut Reader<'_>) -> Result<Value> {
        let held = r.bytes()?;
        Ok(Value::Extension(ExtensionValue { kind: &UNREGISTERED, bytes: Arc::from(held) }))
    }

    fn put_params(&self, _: &DataType, _: &mut Vec<u8>) {}

    fn params(&self, _: &mut Reader<'_>) -> Result<DataType> {
        Err(not_registered("a type's parameters"))
    }

    fn record_width(&self, _: &DataType) -> Option<usize> {
        None
    }

    fn encode_record(&self, _: &Value, _: &DataType) -> Result<Vec<u8>> {
        Err(not_registered("stored"))
    }

    fn decode_record(&self, _: &[u8], _: &DataType) -> Option<Value> {
        None
    }

    fn encode_key(&self, _: &Value, _: &DataType, _: &mut Vec<u8>) -> Result<()> {
        Err(not_registered("a key"))
    }

    fn decode_key(&self, _: &mut &[u8], _: &DataType) -> Result<Value> {
        Err(not_registered("a key"))
    }

    fn anchor(&self, _: &Value, _: &DataType) -> Option<(Value, bool)> {
        None
    }
}
