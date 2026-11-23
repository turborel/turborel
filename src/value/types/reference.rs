//! [ISO/IEC 9075-2:2023 §4.11] A reference type, REF(T): its values each
//! reference a site, a row of a referenceable table, whose most specific
//! type is a subtype of T, its referenced type. Its values are
//! `Value::Reference`, their octets [§4.11.1]: of a system-generated
//! representation, the number a typed table hierarchy gives each row it
//! inserts; of a user-defined one, the value of its representation type as
//! `value::codec` writes it; of a derived one, the derivational attributes'
//! values so written, as a row's (UV103, UA063) (RULES.md D81).
//!
//! Two references compare by their octets alone, equal or not [§8.2 GR9],
//! a reference being in no ordering operation [§9.14 SR3]; they are
//! grouped and are keys' parts by their octets, as a binary string's.

use std::cmp::Ordering;
use std::sync::Arc;

use super::composite::mismatch;
use super::structured::{StructuredType, UdtName, put_names, read_names};
use super::{Descriptor, SqlType};
use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::value::cast::uncastable;
use crate::value::{Binary, Bucket, DataType, Value, unassignable};

/// [ISO/IEC 9075-2:2023 §6.1] `<reference type>` ::= REF ( `<referenced
/// type>` ), its name, the DATA_TYPE of the Information Schema [§20.1,
/// Table 30].
pub const REF_NAME: &str = "REF";

/// [ISO/IEC 9075-2:2023 §4.11.1] (IV071) The most octets a reference
/// has, the length a descriptor and the Information Schema give each
/// reference type: a representation type whose values may need more is
/// refused as it is defined (RULES.md D81).
pub const REF_OCTETS: usize = 1_024;

/// [ISO/IEC 9075-2:2023 §4.11.1] A reference type's descriptor: its
/// referenced type's name, and its scope, the referenceable table its
/// values reference rows of, if it has one [§6.1 GR24].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefType {
    /// The referenced type.
    pub referenced: Arc<StructuredType>,
    /// The scope: the table's schema and name.
    pub scope: Option<UdtName>,
}

impl DataType {
    /// REF(`referenced`) of scope `scope`.
    pub fn reference(referenced: StructuredType, scope: Option<UdtName>) -> DataType {
        DataType::Reference(Arc::new(RefType { referenced: Arc::new(referenced), scope }))
    }
}

/// The kind of every reference type.
#[derive(Debug)]
pub struct ReferenceKind;

/// The one value of it.
pub static REFERENCE: ReferenceKind = ReferenceKind;

/// The reference type `t` is, if it is one.
fn ref_type(t: &DataType) -> Option<&RefType> {
    if let DataType::Reference(r) = t { Some(r) } else { None }
}

/// The octets of `v`, if it is a reference.
fn octets(v: &Value) -> Option<&Binary> {
    if let Value::Reference(b) = v { Some(b) } else { None }
}

impl SqlType for ReferenceKind {
    fn name(&self) -> &'static str {
        REF_NAME
    }

    /// [ISO/IEC 9075-2:2023 §4.11.1] REF(its referenced type's name),
    /// then SCOPE and its table's, if it has one.
    fn written(&self, t: &DataType) -> String {
        match ref_type(t) {
            Some(RefType { referenced, scope: Some(s) }) => format!("{REF_NAME}({}) SCOPE {s}", referenced.name),
            Some(RefType { referenced, scope: None }) => format!("{REF_NAME}({})", referenced.name),
            None => REF_NAME.to_owned(),
        }
    }

    /// [ISO/IEC 9075-2:2023 §4.11.1] With a reference type: whether their
    /// referenced types have a common supertype, the binder knows of the
    /// catalog, as it binds each comparison.
    fn comparable(&self, other: &DataType) -> bool {
        matches!(other, DataType::Reference(_))
    }

    /// [ISO/IEC 9075-2:2023 §8.2 GR9] By their octets.
    fn compare(&self, a: &Value, b: &Value) -> Option<Ordering> {
        Some(octets(a)?.bytes().cmp(octets(b)?.bytes()))
    }

    fn bucket(&self, v: &Value) -> Option<Bucket> {
        Some(Bucket::Binary(octets(v)?.bytes().into()))
    }

    /// [ISO/IEC 9075-2:2023 §9.2 GR3b xxi] The value itself: that its
    /// referenced type is a subtype of the target's, the binder checked
    /// \[SR4\].
    fn assign(&self, v: Value, t: &DataType, rule: &str) -> Result<Value> {
        match (&v, ref_type(t)) {
            (Value::Reference(_), Some(_)) | (Value::Null, _) => Ok(v),
            _ => unassignable(&v, t, rule),
        }
    }

    /// [ISO/IEC 9075-2:2023 §6.13 GR4a] The value itself, of the target's
    /// scope, to a reference type of a compatible referenced type.
    fn cast(&self, v: Value, t: &DataType) -> Result<Value> {
        match (&v, ref_type(t)) {
            (Value::Reference(_), Some(_)) | (Value::Null, _) => Ok(v),
            _ => uncastable(&v, t),
        }
    }

    /// \[DESIGN\] No `<literal>` is of a reference type: REF and its
    /// octets in hexadecimal.
    fn literal(&self, v: &Value) -> String {
        octets(v).map_or_else(|| "NULL".to_owned(), |b| format!("{REF_NAME}({b})"))
    }

    fn text(&self, v: &Value) -> String {
        self.literal(v)
    }

    fn position(&self, _: &Value) -> Option<f64> {
        None
    }

    /// [ISO/IEC 9075-2:2023 §20.1, Table 30] REF, its referenced type the
    /// user-defined type of its descriptor.
    fn descriptor(&self, t: &DataType) -> Descriptor {
        let named = ref_type(t).map(|r| (r.referenced.name.schema.clone(), r.referenced.name.name.clone()));
        Descriptor { user_defined: named, ..Descriptor::of(REF_NAME) }
    }

    fn put_value(&self, v: &Value, out: &mut Vec<u8>) {
        out.put_bytes(octets(v).map_or(&[][..], Binary::bytes));
    }

    fn value(&self, r: &mut Reader<'_>) -> Result<Value> {
        Ok(Value::Reference(Binary::of(r.bytes()?)))
    }

    /// Its referenced type's names (`structured::put_names`), then 0, or 1
    /// and its scope's schema and name.
    fn put_params(&self, t: &DataType, out: &mut Vec<u8>) {
        if let Some(r) = ref_type(t) {
            put_names(&r.referenced.name, &r.referenced.supertypes, out);
            match &r.scope {
                Some(s) => {
                    out.put_u8(1);
                    out.put_str(&s.schema);
                    out.put_str(&s.name);
                }
                None => out.put_u8(0),
            }
        }
    }

    fn params(&self, r: &mut Reader<'_>) -> Result<DataType> {
        let (name, supertypes) = read_names(r)?;
        let referenced = StructuredType { name, supertypes };
        let scope = match r.u8()? {
            0 => None,
            1 => Some(UdtName { schema: r.str()?, name: r.str()? }),
            _ => return Err(r.corrupt("a reference type's scope flag is 0 or 1")),
        };
        Ok(DataType::reference(referenced, scope))
    }

    fn record_width(&self, _: &DataType) -> Option<usize> {
        None
    }

    fn encode_record(&self, v: &Value, t: &DataType) -> Result<Vec<u8>> {
        octets(v).map(|b| b.bytes().to_vec()).ok_or_else(|| mismatch(v, t))
    }

    fn decode_record(&self, field: &[u8], _: &DataType) -> Option<Value> {
        Some(Value::Reference(Binary::of(field)))
    }

    /// Its octets, as a binary string's key.
    fn encode_key(&self, v: &Value, t: &DataType, out: &mut Vec<u8>) -> Result<()> {
        octets(v).ok_or_else(|| mismatch(v, t))?.put_key(out);
        Ok(())
    }

    fn decode_key(&self, rest: &mut &[u8], _: &DataType) -> Result<Value> {
        Binary::take_key(rest).map(Value::Reference).ok_or_else(super::malformed)
    }

    fn anchor(&self, v: &Value, _: &DataType) -> Option<(Value, bool)> {
        octets(v).map(|_| (v.clone(), true))
    }
}

// Unit tests: the kind's text, comparison by octets, CAST and assignment,
// parameters and keys.
#[cfg(test)]
mod tests;
