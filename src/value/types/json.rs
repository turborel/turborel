//! [ISO/IEC 9075-2:2023 §4.8, §6.1] JSON, the JSON type (Feature T801):
//! its values are `Value::Json`, SQL/JSON items in their stored form
//! (`json/binary.rs`, RULES.md D89).
//!
//! [§4.8.2] Every SQL/JSON value is assignable to a site of type JSON and
//! any two are comparable, by their comparison keys (`binary/key.rs`), so
//! values of the type are equal, grouped, ordered and kept in a key
//! (Features T879 to T882). [§6.13 SR, Table 6] CAST takes JSON to JSON
//! alone \[GR22\]. \[EXTENSION\] CAST takes a character string to JSON,
//! parsing its JSON text as JSON ( ) does [§6.40], and JSON to a character
//! string, serializing it as JSON_SERIALIZE does [§6.37]; and a character
//! string is stored in a site of type JSON as CAST makes it, so JSON text
//! is the input form of the type.
//!
//! In a record, the log and the catalog, a value is its stored form; on
//! the wire, its JSON text (`protocol/src/fields.rs`). Its key is its
//! comparison key, which reads back as an equivalent value, not the same
//! one, so no index-only scan gives it (`engine::key::restores`).

use std::cmp::Ordering;

use super::{Descriptor, SqlType};
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError};
use crate::functions::lobs::like::held_text;
use crate::json::Keys;
use crate::json::binary::item_of_key;
use crate::value::cast::uncastable;
use crate::value::json::Json;
use crate::value::lob::LobKind;
use crate::value::{Bucket, DataType, Value, unassignable};

/// [ISO/IEC 9075-2:2023 §6.1] `<JSON type>` ::= JSON, its name. No key
/// word of SQL-92, so a column of SQL-92 named JSON keeps its name.
pub const JSON_NAME: &str = "JSON";

/// The kind of JSON.
#[derive(Debug)]
pub struct JsonKind;

/// The one value of it.
pub static JSON: JsonKind = JsonKind;

/// What a character large object read whole as JSON text is, for the
/// error of one too long.
const JSON_TEXT: &str = "JSON text stored as JSON";

/// The value of JSON `v` is, if it is one.
fn json(v: &Value) -> Option<&Json> {
    if let Value::Json(j) = v { Some(j) } else { None }
}

/// A value not of a column of `t`'s type.
fn mismatch(v: &Value, t: &DataType) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] {v} is not a value of a column of type {t}"))
}

/// \[EXTENSION\] The value of JSON of `v`, a value of JSON or a character
/// string's JSON text, parsed WITHOUT UNIQUE KEYS as JSON ( ) parses it
/// [§6.40 SR4]; none of any other value.
///
/// # Errors
///
/// SQLSTATE 22032 if the text is no JSON text [§9.42 GR3a iii 1]; SQLCODE
/// -900 if it is a character large object longer than a chunk, or beyond
/// turborel's limits.
fn of_text(v: &Value) -> Result<Option<Json>> {
    match v {
        Value::Json(j) => Ok(Some(j.clone())),
        Value::Char(text) => Json::parse(text, Keys::Any).map(Some),
        Value::Lob(lob) => match lob.kind() {
            LobKind::Character => Json::parse(&held_text(lob, JSON_TEXT)?, Keys::Any).map(Some),
            LobKind::Binary => Ok(None),
        },
        Value::Null
        | Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Binary(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Row(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Xml(_)
        | Value::Extension(_) => Ok(None),
    }
}

impl SqlType for JsonKind {
    fn name(&self) -> &'static str {
        JSON_NAME
    }

    fn written(&self, _: &DataType) -> String {
        JSON_NAME.to_owned()
    }

    /// [§4.8.2] Any two SQL/JSON values are comparable; of no other type.
    fn comparable(&self, other: &DataType) -> bool {
        matches!(other, DataType::Json)
    }

    /// [§8.2 GR10] By their comparison keys.
    fn compare(&self, a: &Value, b: &Value) -> Option<Ordering> {
        json(a)?.compare(json(b)?)
    }

    /// Of its comparison key, which equal values share.
    fn bucket(&self, v: &Value) -> Option<Bucket> {
        json(v)?.key().ok().map(|key| Bucket::Binary(key.into()))
    }

    /// [§4.8.2] A value of JSON, to any site of the type; \[EXTENSION\] a
    /// character string, its JSON text parsed.
    fn assign(&self, v: Value, t: &DataType, rule: &str) -> Result<Value> {
        match of_text(&v)? {
            Some(j) => Ok(Value::Json(j)),
            None => unassignable(&v, t, rule),
        }
    }

    /// [§6.13 GR22] JSON to JSON, itself; \[EXTENSION\] a character
    /// string's JSON text, parsed.
    fn cast(&self, v: Value, t: &DataType) -> Result<Value> {
        if v.is_null() {
            return Ok(v);
        }
        match of_text(&v)? {
            Some(j) => Ok(Value::Json(j)),
            None => uncastable(&v, t),
        }
    }

    /// As JSON ( ) of its JSON text, `<JSON parse>` [§6.40], which gives it
    /// back.
    fn literal(&self, v: &Value) -> String {
        let text = json(v).map(ToString::to_string).unwrap_or_default();
        format!("{JSON_NAME}('{}')", text.replace('\'', "''"))
    }

    /// Its JSON text.
    fn text(&self, v: &Value) -> String {
        json(v).map(ToString::to_string).unwrap_or_default()
    }

    fn position(&self, _: &Value) -> Option<f64> {
        None
    }

    /// [§4.8.1] Its name alone.
    fn descriptor(&self, _: &DataType) -> Descriptor {
        Descriptor::of(JSON_NAME)
    }

    /// Its stored form, after its length.
    fn put_value(&self, v: &Value, out: &mut Vec<u8>) {
        out.put_bytes(json(v).map(Json::bytes).unwrap_or_default());
    }

    fn value(&self, r: &mut Reader<'_>) -> Result<Value> {
        Json::of_bytes(r.bytes()?).map(Value::Json)
    }

    fn put_params(&self, _: &DataType, _: &mut Vec<u8>) {}

    fn params(&self, _: &mut Reader<'_>) -> Result<DataType> {
        Ok(DataType::Json)
    }

    /// Of variable length.
    fn record_width(&self, _: &DataType) -> Option<usize> {
        None
    }

    /// Its stored form.
    fn encode_record(&self, v: &Value, t: &DataType) -> Result<Vec<u8>> {
        json(v).map(|j| j.bytes().to_vec()).ok_or_else(|| mismatch(v, t))
    }

    fn decode_record(&self, field: &[u8], _: &DataType) -> Option<Value> {
        Json::of_bytes(field).ok().map(Value::Json)
    }

    /// Its comparison key, which no other value's key begins with.
    fn encode_key(&self, v: &Value, t: &DataType, out: &mut Vec<u8>) -> Result<()> {
        let j = json(v).ok_or_else(|| mismatch(v, t))?;
        out.extend_from_slice(&j.key()?);
        Ok(())
    }

    /// A value equivalent to the one whose key begins `rest`.
    fn decode_key(&self, rest: &mut &[u8], _: &DataType) -> Result<Value> {
        let item = item_of_key(rest)?;
        Json::of_item(&item).map(Value::Json)
    }

    /// Itself: every SQL/JSON value is one of the column's.
    fn anchor(&self, v: &Value, _: &DataType) -> Option<(Value, bool)> {
        json(v).map(|_| (v.clone(), true))
    }
}
