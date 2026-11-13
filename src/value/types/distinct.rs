//! [ISO/IEC 9075-2:2023 §4.9.2] A distinct type: a user-defined type
//! whose representation is a predefined type or an array type, its source
//! type [§11.51 SR5a, SR7a].
//!
//! \[DESIGN\] A value of a distinct type is its source type's value, as
//! the implementation-dependent representation [§11.51 GR2b] (UV103) is
//! left to turborel: `Value` carries no type, and the binder alone tells a
//! distinct type's values from its source's, as their declared types
//! differ. Its bytes in a record and a key, its hash and its order are its
//! source's, as ORDER FULL BY MAP of its cast to the source orders it
//! [GR2c] (RULES.md D69).

use std::cmp::Ordering;
use std::sync::Arc;

use super::{Descriptor, SqlType, malformed, tags};
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError};
use crate::value::cast::uncastable;
use crate::value::{Bucket, DataType, Value, unassignable};

/// [ISO/IEC 9075-2:2023 §20.1, Table 30] The DATA_TYPE of a user-defined
/// type in the Information Schema, and the name of the kind of a distinct
/// type.
pub const USER_DEFINED_NAME: &str = "USER-DEFINED";

/// [ISO/IEC 9075-2:2023 §4.9.2] A distinct type's descriptor, as a type
/// declared of it holds it: its name, and its source type. It is FINAL
/// and never altered [§11.51 SR7b], so a copy is never stale.
#[derive(Debug, Clone, PartialEq)]
pub struct DistinctType {
    /// The `<authorization identifier>` of its schema.
    pub schema: String,
    /// Its `<identifier>` in that schema.
    pub name: String,
    /// Its source type [§11.51 SR7a]: a predefined type or an array type.
    pub source: DataType,
}

impl DataType {
    /// The distinct type `name` of schema `schema`, of source type `source`.
    pub fn distinct(schema: &str, name: &str, source: DataType) -> DataType {
        DataType::Distinct(Arc::new(DistinctType { schema: schema.to_owned(), name: name.to_owned(), source }))
    }

    /// The type its values are held as: a distinct type's source type
    /// (RULES.md D69), any other type itself.
    pub fn represented(&self) -> &DataType {
        if let DataType::Distinct(d) = self { &d.source } else { self }
    }
}

/// The kind of every distinct type.
#[derive(Debug)]
pub struct DistinctKind;

/// The one value of it.
pub static DISTINCT: DistinctKind = DistinctKind;

/// The distinct type `t` is, if it is one.
fn distinct_type(t: &DataType) -> Option<&DistinctType> {
    if let DataType::Distinct(d) = t { Some(d) } else { None }
}

/// That `t` is not a distinct type, as a type of its kind is.
fn not_distinct(t: &DataType) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] {t} is not a distinct type"))
}

impl SqlType for DistinctKind {
    fn name(&self) -> &'static str {
        USER_DEFINED_NAME
    }

    /// [ISO/IEC 9075-2:2023 §6.1] Its `<path-resolved user-defined type
    /// name>`, its schema's name made explicit.
    fn written(&self, t: &DataType) -> String {
        distinct_type(t).map_or_else(|| USER_DEFINED_NAME.to_owned(), |d| format!("{}.{}", d.schema, d.name))
    }

    /// [ISO/IEC 9075-2:2023 §4.9.5] With itself alone, as no other type is
    /// of its subtype family, and only if it has a comparison type: its
    /// ordering is its source's, which no array type has [§11.51 GR2c].
    /// As a kind is told only the other type, `DataType::comparable` asks
    /// this of both.
    fn comparable(&self, other: &DataType) -> bool {
        distinct_type(other).is_some_and(|d| !matches!(d.source, DataType::Array(_)))
    }

    /// [ISO/IEC 9075-2:2023 §11.51 GR2c] As their source values compare.
    fn compare(&self, a: &Value, b: &Value) -> Option<Ordering> {
        crate::value::compare(a, b)
    }

    fn bucket(&self, v: &Value) -> Option<Bucket> {
        crate::value::bucket(v)
    }

    /// [ISO/IEC 9075-2:2023 §4.13, §11.51 GR2b] Its cast function's
    /// parameter assigned: the value assigned to the source type.
    fn assign(&self, v: Value, t: &DataType, rule: &str) -> Result<Value> {
        match distinct_type(t) {
            Some(d) => crate::value::assign(v, &d.source, rule),
            None => unassignable(&v, t, rule),
        }
    }

    /// [ISO/IEC 9075-2:2023 §6.13 GR3b] Its cast function invoked: the
    /// value cast to the source type, which the binder allows only of the
    /// source type or one whose precedence list holds it \[SR12\].
    fn cast(&self, v: Value, t: &DataType) -> Result<Value> {
        match distinct_type(t) {
            Some(d) => crate::value::cast(v, &d.source),
            None => uncastable(&v, t),
        }
    }

    fn literal(&self, v: &Value) -> String {
        v.kind().map_or_else(|| "NULL".to_owned(), |k| k.literal(v))
    }

    fn text(&self, v: &Value) -> String {
        v.kind().map(|k| k.text(v)).unwrap_or_default()
    }

    fn position(&self, v: &Value) -> Option<f64> {
        v.kind()?.position(v)
    }

    /// [ISO/IEC 9075-2:2023 §4.9.2] USER-DEFINED, and its name: the
    /// descriptor of its source is the type's own [§11.51 GR2a].
    fn descriptor(&self, t: &DataType) -> Descriptor {
        let named = distinct_type(t).map(|d| (d.schema.clone(), d.name.clone()));
        Descriptor { user_defined: named, ..Descriptor::of(USER_DEFINED_NAME) }
    }

    fn put_value(&self, v: &Value, out: &mut Vec<u8>) {
        crate::value::codec::put_value(out, v);
    }

    fn value(&self, r: &mut Reader<'_>) -> Result<Value> {
        crate::value::codec::value(r)
    }

    /// Its schema's name and its own, then its source type.
    fn put_params(&self, t: &DataType, out: &mut Vec<u8>) {
        if let Some(d) = distinct_type(t) {
            out.put_str(&d.schema);
            out.put_str(&d.name);
            tags::put_type(out, &d.source);
        }
    }

    fn params(&self, r: &mut Reader<'_>) -> Result<DataType> {
        r.nested(|r| {
            let (schema, name) = (r.str()?, r.str()?);
            let source = tags::data_type(r)?;
            if matches!(source, DataType::Distinct(_) | DataType::Row(_)) {
                return Err(r.corrupt("a distinct type's source is a predefined type or an array type"));
            }
            Ok(DataType::distinct(&schema, &name, source))
        })
    }

    fn record_width(&self, t: &DataType) -> Option<usize> {
        let d = distinct_type(t)?;
        d.source.kind().record_width(&d.source)
    }

    fn encode_record(&self, v: &Value, t: &DataType) -> Result<Vec<u8>> {
        let d = distinct_type(t).ok_or_else(|| not_distinct(t))?;
        d.source.encode_record(v)
    }

    fn decode_record(&self, field: &[u8], t: &DataType) -> Option<Value> {
        distinct_type(t)?.source.decode_record(field)
    }

    fn encode_key(&self, v: &Value, t: &DataType, out: &mut Vec<u8>) -> Result<()> {
        let d = distinct_type(t).ok_or_else(|| not_distinct(t))?;
        d.source.encode_key(v, out)
    }

    fn decode_key(&self, rest: &mut &[u8], t: &DataType) -> Result<Value> {
        distinct_type(t).ok_or_else(malformed)?.source.decode_key(rest)
    }

    fn anchor(&self, v: &Value, t: &DataType) -> Option<(Value, bool)> {
        let d = distinct_type(t)?;
        d.source.kind().anchor(v, &d.source)
    }
}
