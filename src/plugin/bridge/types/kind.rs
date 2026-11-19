//! \[EXTENSION\] What a plug-in's type does with its values (RULES.md
//! D223), each read from the forms its value keeps: no call but a cast of
//! a character or binary string to it.

use std::cmp::Ordering;
use std::sync::Arc;

use super::{PluginType, forms};
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError};
use crate::value::cast::uncastable;
use crate::value::types::{Descriptor, ExtensionValue};
use crate::value::{Binary, Bucket, DataType, SqlType, Value, unassignable};

/// The bytes of `v`, a value of `t`.
fn bytes_of<'v>(t: &PluginType, v: &'v Value) -> Option<&'v [u8]> {
    let Value::Extension(e) = v else { return None };
    (e.kind.name() == t.name).then_some(e.bytes.as_ref())
}

impl SqlType for PluginType {
    fn name(&self) -> &str {
        self.name
    }
    fn written(&self, _: &DataType) -> String {
        self.name.to_owned()
    }
    fn comparable(&self, other: &DataType) -> bool {
        if let DataType::Extension(e) = other { e.kind.name() == self.name } else { false }
    }
    fn compare(&self, a: &Value, b: &Value) -> Option<Ordering> {
        Some(self.forms_of(a)?.key.cmp(&self.forms_of(b)?.key))
    }
    fn bucket(&self, v: &Value) -> Option<Bucket> {
        Some(Bucket::Extension(self.forms_of(v)?.key.into()))
    }
    fn assign(&self, v: Value, t: &DataType, rule: &str) -> Result<Value> {
        if v.is_null() || bytes_of(self, &v).is_some() { Ok(v) } else { unassignable(&v, t, rule) }
    }
    fn assigns_text(&self) -> bool {
        true
    }
    fn cast(&self, v: Value, t: &DataType) -> Result<Value> {
        if v.is_null() || bytes_of(self, &v).is_some() {
            return Ok(v);
        }
        match &v {
            Value::Char(text) => self.input(text),
            Value::Binary(b) => self.receive(b.bytes()),
            Value::Exact(_)
            | Value::Approx(_)
            | Value::DecFloat(_)
            | Value::Bool(_)
            | Value::Datetime(_)
            | Value::Interval(_)
            | Value::Lob(_)
            | Value::Array(_)
            | Value::Multiset(_)
            | Value::Row(_)
            | Value::Structured(_)
            | Value::Reference(_)
            | Value::Json(_)
            | Value::Xml(_)
            | Value::Extension(_)
            | Value::Null => uncastable(&v, t),
        }
    }
    fn lowered(&self, v: &Value, to: &DataType) -> Option<Value> {
        let m = self.forms_of(v)?;
        if to.is_character() {
            return Some(Value::Char(m.text));
        }
        matches!(to, DataType::Binary { .. } | DataType::VarBinary { .. }).then(|| Value::Binary(Binary::new(m.binary)))
    }
    fn literal(&self, v: &Value) -> String {
        let text = self.forms_of(v).map(|m| m.text).unwrap_or_default();
        format!("CAST('{}' AS {})", text.replace('\'', "''"), self.name)
    }
    fn text(&self, v: &Value) -> String {
        self.forms_of(v).map(|m| m.text).unwrap_or_default()
    }
    fn position(&self, _: &Value) -> Option<f64> {
        None
    }
    fn descriptor(&self, _: &DataType) -> Descriptor {
        Descriptor::of(self.name)
    }
    fn put_value(&self, v: &Value, out: &mut Vec<u8>) {
        out.put_bytes(bytes_of(self, v).unwrap_or_default());
    }
    fn value(&self, r: &mut Reader<'_>) -> Result<Value> {
        let raw = r.bytes()?;
        if forms(raw).is_none() {
            return Err(r.corrupt(&format!("an invalid {} value", self.name)));
        }
        Ok(self.of_bytes(raw))
    }
    fn ordered(&self) -> bool {
        self.ordered
    }
    fn put_params(&self, _: &DataType, _: &mut Vec<u8>) {}
    fn params(&self, _: &mut Reader<'_>) -> Result<DataType> {
        Ok(self.data_type())
    }
    fn record_width(&self, _: &DataType) -> Option<usize> {
        None
    }
    fn encode_record(&self, v: &Value, _: &DataType) -> Result<Vec<u8>> {
        bytes_of(self, v).map(<[u8]>::to_vec).ok_or_else(|| self.mismatch(v))
    }
    fn decode_record(&self, field: &[u8], _: &DataType) -> Option<Value> {
        forms(field)?;
        Some(self.of_bytes(field))
    }
    fn encode_key(&self, v: &Value, _: &DataType, out: &mut Vec<u8>) -> Result<()> {
        let m = self.forms_of(v).ok_or_else(|| self.mismatch(v))?;
        Binary::new(m.key).put_key(out);
        Binary::new(bytes_of(self, v).unwrap_or_default().to_vec()).put_key(out);
        Ok(())
    }
    fn decode_key(&self, rest: &mut &[u8], _: &DataType) -> Result<Value> {
        let corrupt = || SqlError::new(Code::Corrupt, format!("[DESIGN] an invalid {} index key", self.name));
        Binary::take_key(rest).ok_or_else(corrupt)?;
        let whole = Binary::take_key(rest).ok_or_else(corrupt)?;
        forms(whole.bytes()).map(|_| self.of_bytes(whole.bytes())).ok_or_else(corrupt)
    }
    fn anchor(&self, v: &Value, _: &DataType) -> Option<(Value, bool)> {
        bytes_of(self, v).map(|_| (v.clone(), true))
    }
}

impl PluginType {
    /// The value whose bytes are `bytes`, of this type as the process
    /// keeps it.
    fn of_bytes(&self, bytes: &[u8]) -> Value {
        Value::Extension(ExtensionValue { kind: *self.kind(), bytes: Arc::from(bytes) })
    }
}
