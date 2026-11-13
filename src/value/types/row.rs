//! [ISO/IEC 9075-2:2023 §4.10] ROW: a sequence of fields, each a name and
//! a value of its declared type. Its values are `Value::Row`, its fields'
//! values in order.
//!
//! A row's bytes in a record are its fields', as `composite` writes them;
//! its key is its fields' keys, each after a byte saying whether it is
//! null, so keys order as the rows do field by field (RULES.md D68).

use std::cmp::Ordering;
use std::sync::Arc;

use super::composite::{self, mismatch};
use super::{Descriptor, SqlType, malformed, tags, take};
use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::value::cast::{cast, uncastable};
use crate::value::{Bucket, DataType, Value, assign, unassignable};

/// [ISO/IEC 9075-2:2023 §6.1] `<row type>` ::= ROW `<row type body>`, its
/// name.
pub const ROW_NAME: &str = "ROW";

/// The key byte of a field that is null, and of one present.
const NULL: u8 = 0;
const PRESENT: u8 = 1;

/// [ISO/IEC 9075-2:2023 §6.2 GR3] A field's descriptor: its name and its
/// declared type; its ordinal position is its place among its row type's.
#[derive(Debug, Clone)]
pub struct Field {
    /// The `<field name>`.
    pub name: String,
    /// The declared type.
    pub data_type: DataType,
    /// Whether the name is implementation-dependent (UV077), made for a
    /// field no `<field name>` names, as a dynamic descriptor's UNNAMED
    /// shows [§20.9 GR7c iii 2 A]. A type kept in the catalog or sent to a
    /// client names its fields, so it is not kept there.
    pub unnamed: bool,
}

/// Two fields are alike by name and type alone: a name made for a field
/// is still its name [§4.10.2].
impl PartialEq for Field {
    fn eq(&self, other: &Field) -> bool {
        self.name == other.name && self.data_type == other.data_type
    }
}

/// [ISO/IEC 9075-2:2023 §4.10] A row type's descriptor: its fields', in
/// order, one or more.
#[derive(Debug, Clone, PartialEq)]
pub struct RowType {
    /// The fields.
    pub fields: Vec<Field>,
}

impl DataType {
    /// The row type of `fields`.
    pub fn row(fields: Vec<Field>) -> DataType {
        DataType::Row(Arc::new(RowType { fields }))
    }
}

/// The kind of ROW.
#[derive(Debug)]
pub struct RowKind;

/// The one value of it.
pub static ROW: RowKind = RowKind;

/// The row type `t` is, if it is one.
fn row_type(t: &DataType) -> Option<&RowType> {
    if let DataType::Row(r) = t { Some(r) } else { None }
}

/// The fields' values of `v`, if it is a row.
fn fields(v: &Value) -> Option<&[Value]> {
    if let Value::Row(f) = v { Some(f) } else { None }
}

/// The values of `v`, a row of `t`'s degree, each with its field's type.
fn paired<'a>(v: &'a Value, t: &'a DataType) -> Option<impl Iterator<Item = (&'a Value, &'a DataType)>> {
    let (r, given) = (row_type(t)?, fields(v)?);
    (r.fields.len() == given.len()).then(|| given.iter().zip(r.fields.iter().map(|f| &f.data_type)))
}

impl SqlType for RowKind {
    fn name(&self) -> &'static str {
        ROW_NAME
    }

    /// [ISO/IEC 9075-2:2023 §6.1] ROW, then each `<field definition>`.
    fn written(&self, t: &DataType) -> String {
        let Some(r) = row_type(t) else { return ROW_NAME.to_owned() };
        let fields: Vec<String> = r.fields.iter().map(|f| format!("{} {}", f.name, f.data_type)).collect();
        format!("{ROW_NAME}({})", fields.join(", "))
    }

    /// [ISO/IEC 9075-2:2023 §4.10] Rows with rows alone; of one degree,
    /// field by field comparable, `DataType::comparable` says.
    fn comparable(&self, other: &DataType) -> bool {
        matches!(other, DataType::Row(_))
    }

    /// [ISO/IEC 9075-2:2023 §8.2 GR1b i] As `value::compare` says.
    fn compare(&self, a: &Value, b: &Value) -> Option<Ordering> {
        crate::value::compare(a, b)
    }

    fn bucket(&self, v: &Value) -> Option<Bucket> {
        composite::bucket_parts(fields(v)?)
    }

    /// [ISO/IEC 9075-2:2023 §9.2 GR3b xx] Field by field.
    fn assign(&self, v: Value, t: &DataType, rule: &str) -> Result<Value> {
        let Some(pairs) = paired(&v, t) else { return unassignable(&v, t, rule) };
        let assigned = pairs.map(|(f, ft)| assign(f.clone(), ft, rule)).collect::<Result<Vec<_>>>()?;
        Ok(Value::Row(assigned.into()))
    }

    /// [ISO/IEC 9075-2:2023 §6.13 GR7] Field by field.
    fn cast(&self, v: Value, t: &DataType) -> Result<Value> {
        let Some(pairs) = paired(&v, t) else {
            return if v.is_null() { Ok(v) } else { uncastable(&v, t) };
        };
        let cast = pairs.map(|(f, ft)| cast(f.clone(), ft)).collect::<Result<Vec<_>>>()?;
        Ok(Value::Row(cast.into()))
    }

    /// [ISO/IEC 9075-2:2023 §7.1] `<explicit row value constructor>` of its
    /// fields' literals.
    fn literal(&self, v: &Value) -> String {
        format!("{ROW_NAME}({})", fields(v).map(composite::literals).unwrap_or_default())
    }

    fn text(&self, v: &Value) -> String {
        self.literal(v)
    }

    fn position(&self, _: &Value) -> Option<f64> {
        None
    }

    fn descriptor(&self, _: &DataType) -> Descriptor {
        Descriptor::of(ROW_NAME)
    }

    fn put_value(&self, v: &Value, out: &mut Vec<u8>) {
        composite::put_parts(fields(v).unwrap_or_default(), out);
    }

    fn value(&self, r: &mut Reader<'_>) -> Result<Value> {
        composite::read_parts(r).map(Value::Row)
    }

    /// Its degree, then each field's name and type.
    fn put_params(&self, t: &DataType, out: &mut Vec<u8>) {
        let fields = row_type(t).map_or(&[][..], |r| &r.fields);
        out.put_u32(u32::try_from(fields.len()).unwrap_or(u32::MAX));
        for f in fields {
            out.put_str(&f.name);
            tags::put_type(out, &f.data_type);
        }
    }

    fn params(&self, r: &mut Reader<'_>) -> Result<DataType> {
        r.nested(|r| {
            let degree = r.u32()?;
            // Pushed one by one, so a corrupt degree fails at the first
            // field missing.
            let mut fields = Vec::new();
            for _ in 0..degree {
                fields.push(Field { name: r.str()?, data_type: tags::data_type(r)?, unnamed: false });
            }
            if fields.is_empty() {
                return Err(r.corrupt("a row type has no fields"));
            }
            Ok(DataType::row(fields))
        })
    }

    fn record_width(&self, _: &DataType) -> Option<usize> {
        None
    }

    fn encode_record(&self, v: &Value, t: &DataType) -> Result<Vec<u8>> {
        let (Some(r), Some(given)) = (row_type(t), fields(v)) else { return Err(mismatch(v, t)) };
        if given.len() != r.fields.len() {
            return Err(mismatch(v, t));
        }
        composite::encode_parts(given, |i| r.fields.get(i).map(|f| &f.data_type), t)
    }

    fn decode_record(&self, field: &[u8], t: &DataType) -> Option<Value> {
        let r = row_type(t)?;
        composite::decode_parts(field, |i| r.fields.get(i).map(|f| &f.data_type)).filter(|f| f.len() == r.fields.len()).map(Value::Row)
    }

    /// Each field's key, after whether it is null.
    fn encode_key(&self, v: &Value, t: &DataType, out: &mut Vec<u8>) -> Result<()> {
        let Some(pairs) = paired(v, t) else { return Err(mismatch(v, t)) };
        for (f, ft) in pairs {
            if f.is_null() {
                out.push(NULL);
            } else {
                out.push(PRESENT);
                ft.encode_key(f, out)?;
            }
        }
        Ok(())
    }

    fn decode_key(&self, rest: &mut &[u8], t: &DataType) -> Result<Value> {
        let r = row_type(t).ok_or_else(malformed)?;
        let mut values = Vec::with_capacity(r.fields.len());
        for f in &r.fields {
            values.push(match take(rest, 1)? {
                [NULL] => Value::Null,
                [PRESENT] => f.data_type.decode_key(rest)?,
                _ => return Err(malformed()),
            });
        }
        Ok(Value::Row(values.into()))
    }

    /// Each field's anchor, if every field has one that is itself; else
    /// none, so every key is compared with it: as of a row with a null
    /// field, which compares with no key.
    fn anchor(&self, v: &Value, t: &DataType) -> Option<(Value, bool)> {
        let mut anchored = Vec::new();
        for (f, ft) in paired(v, t)? {
            let (at, exact) = ft.kind().anchor(f, ft)?;
            if !exact {
                return None;
            }
            anchored.push(at);
        }
        Some((Value::Row(anchored.into()), true))
    }
}
