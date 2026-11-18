//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15] The thirteen linear
//! referencing types as kinds of type built in (RULES.md D53), as VECTOR
//! and FULLTEXT are, of no family: a value's bytes are its stored form
//! (`LrValue::encode`), its most specific type in them.
//!
//! [§15.3.1, §15.4.1, §15.5.1] ST_LRFeature, ST_LRCurve and
//! ST_LRDirectedEdge are UNDER ST_LinearElement, so a value of each is
//! assigned to a site of ST_LinearElement as it is [ISO/IEC 9075-2:2023
//! §9.2 SR2]. The types declare no ordering, so their values are
//! compared by no operator, sorted, grouped nor keyed; the bytes are
//! compared only where the engine keeps them.

use std::cmp::Ordering;
use std::sync::Arc;

use super::super::{ALL, KINDS, LrKind, LrValue};
use super::text_of;
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::value::types::{Descriptor, ExtensionType, ExtensionValue, Kind as TypeKind, TypeParams};
use crate::value::{Bucket, DataType, SqlType, Value};

/// A linear referencing type as a kind of type.
#[derive(Debug)]
pub struct LrType {
    kind: LrKind,
}

/// The types, in the order of `ALL`.
static TYPES: [LrType; KINDS] = [
    LrType { kind: LrKind::Lrm },
    LrType { kind: LrKind::LinearElement },
    LrType { kind: LrKind::LrFeature },
    LrType { kind: LrKind::LrCurve },
    LrType { kind: LrKind::LrDirectedEdge },
    LrType { kind: LrKind::PositionExp },
    LrType { kind: LrKind::LrMeasure },
    LrType { kind: LrKind::StartValue },
    LrType { kind: LrKind::DistanceExp },
    LrType { kind: LrKind::Referent },
    LrType { kind: LrKind::LatOffsetExp },
    LrType { kind: LrKind::VerOffsetExp },
    LrType { kind: LrKind::VectorOffsetExp },
];

/// The types as kinds of type.
static REFS: [&dyn SqlType; KINDS] = [
    &TYPES[0], &TYPES[1], &TYPES[2], &TYPES[3], &TYPES[4], &TYPES[5], &TYPES[6], &TYPES[7], &TYPES[8], &TYPES[9], &TYPES[10], &TYPES[11],
    &TYPES[12],
];

/// The types as a type holds them.
static KIND_OF: [TypeKind; KINDS] =
    [&REFS[0], &REFS[1], &REFS[2], &REFS[3], &REFS[4], &REFS[5], &REFS[6], &REFS[7], &REFS[8], &REFS[9], &REFS[10], &REFS[11], &REFS[12]];

/// The kinds of type, in the order of `ALL`, as the type registry holds
/// them.
pub fn kinds() -> &'static [TypeKind] {
    &KIND_OF
}

/// The kind of type of `kind`.
fn kind_of(kind: LrKind) -> TypeKind {
    let [first, ..] = KIND_OF;
    let at = ALL.iter().position(|k| *k == kind).unwrap_or_default();
    KIND_OF.get(at).copied().unwrap_or(first)
}

/// The data type of `kind`.
pub fn data_type_of(kind: LrKind) -> DataType {
    DataType::Extension(ExtensionType { kind: kind_of(kind), params: TypeParams::default() })
}

/// The linear referencing type of the data type `t`, if it is one.
pub fn of_data_type(t: &DataType) -> Option<LrKind> {
    let DataType::Extension(e) = t else { return None };
    let held: &dyn SqlType = *e.kind;
    TYPES.iter().find(|ty| std::ptr::addr_eq(held, *ty)).map(|ty| ty.kind)
}

/// The stored form of `v`, if it is a value of a linear referencing type.
pub fn payload(v: &Value) -> Option<&[u8]> {
    let Value::Extension(e) = v else { return None };
    TYPES.iter().any(|ty| std::ptr::addr_eq(e.kind, ty)).then_some(e.bytes.as_ref())
}

/// The most specific type of `v`, a value of a linear referencing type.
pub fn kind_of_value(v: &Value) -> Option<LrKind> {
    LrKind::of_code(*payload(v)?.get(1)?)
}

/// `v` as a value of its most specific type.
///
/// # Errors
///
/// SQLCODE -900 of a value nested past its depth limit.
pub fn to_value(v: &LrValue) -> Result<Value> {
    let bytes = v.encode()?;
    let kind: &'static dyn SqlType = *kind_of(v.kind());
    Ok(Value::Extension(ExtensionValue { kind, bytes: Arc::from(bytes) }))
}

/// The value `v` holds, a value of a linear referencing type.
///
/// # Errors
///
/// SQLCODE -301 if it is none; -916 of bytes `encode` did not write.
pub fn of_value(v: &Value) -> Result<LrValue> {
    let bytes = payload(v).ok_or_else(|| SqlError::new(Code::Data, "[DESIGN] expected a linear referencing value"))?;
    LrValue::decode(bytes)
}

/// SQLCODE -900: no key is of `t`.
fn unkeyed(t: &DataType) -> SqlError {
    SqlError::new(Code::Unsupported, format!("[DESIGN] no key is of {t}: the linear referencing types declare no ordering"))
}

impl LrType {
    /// That `v` is a value of this type or of a subtype of it [ISO/IEC
    /// 9075-2:2023 §9.2 SR2].
    fn fits(&self, v: &Value) -> Result<()> {
        let kind = kind_of_value(v)
            .ok_or_else(|| SqlError::new(Code::Data, format!("[DESIGN] {v} is not a value of {}", self.kind.sql_name())))?;
        if kind.is_subtype_of(self.kind) {
            return Ok(());
        }
        Err(SqlError::new(
            Code::Data,
            format!("[ISO/IEC 9075-2:2023 §9.2 SR2] a value of {} is not a value of {}", kind.sql_name(), self.kind.sql_name()),
        )
        .with_state(SqlState::SPATIAL_ELEMENT_IS_NOT_A_VALID_TYPE))
    }

    /// The value of the stored form `bytes`, checked to be of this type.
    fn stored(&self, bytes: &[u8]) -> Option<Value> {
        let v = LrValue::decode(bytes).ok()?;
        v.kind().is_subtype_of(self.kind).then(|| to_value(&v).ok()).flatten()
    }
}

impl SqlType for LrType {
    fn name(&self) -> &str {
        self.kind.type_name()
    }

    fn written(&self, _: &DataType) -> String {
        self.kind.type_name().to_owned()
    }

    /// A value of `other` is assigned to a site of this type if it is
    /// this type or a subtype of it.
    fn comparable(&self, other: &DataType) -> bool {
        of_data_type(other).is_some_and(|k| k.is_subtype_of(self.kind))
    }

    fn compare(&self, a: &Value, b: &Value) -> Option<Ordering> {
        Some(payload(a)?.cmp(payload(b)?))
    }

    fn bucket(&self, v: &Value) -> Option<Bucket> {
        Some(Bucket::Extension(payload(v)?.into()))
    }

    fn assign(&self, v: Value, _: &DataType, _: &str) -> Result<Value> {
        if v.is_null() {
            return Ok(v);
        }
        if let Some(text) = text_of(&v)? {
            return self.cast(Value::Char(text), &data_type_of(self.kind));
        }
        self.fits(&v)?;
        Ok(v)
    }

    /// A character string is stored in it as its constructor of one
    /// CHARACTER LARGE OBJECT reads it [§15.1.2 Description 2 and its
    /// peers].
    fn assigns_text(&self) -> bool {
        true
    }

    /// A character string casts to the value of its well-known text or
    /// GML representation; a value of this type or a subtype to itself.
    fn cast(&self, v: Value, _: &DataType) -> Result<Value> {
        if v.is_null() {
            return Ok(v);
        }
        if let Some(text) = text_of(&v)? {
            return to_value(&super::read_text(&text, self.kind)?);
        }
        self.fits(&v)?;
        Ok(v)
    }

    fn ordered(&self) -> bool {
        false
    }

    fn literal(&self, v: &Value) -> String {
        let text = self.text(v);
        format!("CAST('{}' AS {})", text.replace('\'', "''"), self.kind.type_name())
    }

    fn text(&self, v: &Value) -> String {
        of_value(v).and_then(|lr| super::super::as_text(&lr)).unwrap_or_default()
    }

    fn position(&self, _: &Value) -> Option<f64> {
        None
    }

    fn descriptor(&self, _: &DataType) -> Descriptor {
        Descriptor::of(self.kind.type_name())
    }

    fn put_value(&self, v: &Value, out: &mut Vec<u8>) {
        out.put_bytes(payload(v).unwrap_or_default());
    }

    fn value(&self, r: &mut Reader<'_>) -> Result<Value> {
        let raw = r.bytes()?;
        self.stored(raw).ok_or_else(|| r.corrupt(&format!("an invalid {} value", self.kind.sql_name())))
    }

    fn put_params(&self, _: &DataType, _: &mut Vec<u8>) {}

    fn params(&self, _: &mut Reader<'_>) -> Result<DataType> {
        Ok(data_type_of(self.kind))
    }

    fn record_width(&self, _: &DataType) -> Option<usize> {
        None
    }

    fn encode_record(&self, v: &Value, _: &DataType) -> Result<Vec<u8>> {
        self.fits(v)?;
        Ok(payload(v).unwrap_or_default().to_vec())
    }

    fn decode_record(&self, field: &[u8], _: &DataType) -> Option<Value> {
        self.stored(field)
    }

    fn encode_key(&self, _: &Value, t: &DataType, _: &mut Vec<u8>) -> Result<()> {
        Err(unkeyed(t))
    }

    fn decode_key(&self, _: &mut &[u8], t: &DataType) -> Result<Value> {
        Err(unkeyed(t))
    }

    fn anchor(&self, v: &Value, _: &DataType) -> Option<(Value, bool)> {
        self.fits(v).ok().map(|()| (v.clone(), true))
    }
}
