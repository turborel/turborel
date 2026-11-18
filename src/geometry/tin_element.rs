//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §17.1] The ST_TINElement type:
//! a TIN element's type, identifier, tag and geometry, which ST_TIN is
//! built from and ST_TINElements returns. Built in as a kind of type of
//! its own (RULES.md D53), its value a version octet, an octet of the
//! parts it has, then of those the identifier, the type and the tag each
//! after its length, and the geometry's payload. The constructor
//! function's value has none, the empty set [§17.1.1 Description 3]. It
//! has no ordering, so no key [§17.1.1].

use std::cmp::Ordering;
use std::sync::Arc;

use super::model::{Body as ModelBody, Geom, Kind, TinElement};
use super::representations::spatial;
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::spatial::curves::tin::ElementKind;
use crate::spatial::formats::wkt;
use crate::value::types::{Descriptor, ExtensionType, ExtensionValue, TypeParams};
use crate::value::{Bucket, DataType, SqlType, Value};

/// The type's name.
pub const ST_TINELEMENT: &str = "ST_TINELEMENT";
/// The payload's version.
const VERSION: u8 = 1;
/// The octets of the identifier.
const ID_BYTES: usize = 4;
/// The longest element type [§17.1.1].
pub const TYPE_LENGTH: usize = 30;
/// The longest tag [§17.1.1].
pub const TAG_LENGTH: usize = 64;

/// The ST_TINELEMENT type.
#[derive(Debug)]
pub struct TinElementType;

/// The kind of ST_TINElement values.
pub static TIN_ELEMENT_TYPE: TinElementType = TinElementType;

/// It, as a type of it holds it.
static TIN_ELEMENT_KIND: &dyn SqlType = &TIN_ELEMENT_TYPE;

/// The kind of ST_TINELEMENT, as the type registry holds it.
pub fn kind() -> &'static &'static dyn SqlType {
    &TIN_ELEMENT_KIND
}

/// The ST_TINELEMENT type.
pub fn data_type() -> DataType {
    DataType::Extension(ExtensionType { kind: &TIN_ELEMENT_KIND, params: TypeParams::default() })
}

/// The payload of `v`, if it is an ST_TINElement value.
fn payload(v: &Value) -> Option<&[u8]> {
    let Value::Extension(e) = v else { return None };
    (e.kind.name() == ST_TINELEMENT).then_some(e.bytes.as_ref())
}

/// The geometry type an element type takes, and whether it takes z alone
/// [§17.1.2 Description 2a, 2b].
fn takes(kind: ElementKind) -> (Kind, bool) {
    match kind {
        ElementKind::RandomPoints | ElementKind::GroupSpot => (Kind::MultiPoint, true),
        ElementKind::Boundary | ElementKind::BreakVoid | ElementKind::Void => (Kind::Polygon, true),
        ElementKind::DrapeVoid | ElementKind::Hole => (Kind::Polygon, false),
        ElementKind::Breakline | ElementKind::SoftBreak | ElementKind::ControlContour => (Kind::LineString, true),
        ElementKind::StopLine => (Kind::LineString, false),
    }
}

/// The flags of the parts a payload holds.
const HAS_TYPE: u8 = 1;
const HAS_ID: u8 = 2;
const HAS_TAG: u8 = 4;
const HAS_GEOMETRY: u8 = 8;

/// [§17.1.2] The element of type `element_type`, identifier `id`, tag
/// `tag` and geometry `g`, checked.
///
/// # Errors
///
/// SQLSTATE 2FF02 of an unknown type or a tag longer than 64
/// characters; 2FF74 of a geometry the type does not take.
pub fn element(element_type: &str, id: i32, tag: &str, g: &Geom) -> Result<Value> {
    Parts { element_type: Some(element_type.to_owned()), id: Some(id), tag: Some(tag.to_owned()), geometry: Some(g.clone()) }.to_value()
}

/// [§17.1.1 Description 3] The value the constructor function returns,
/// the empty set.
pub fn empty() -> Value {
    Value::Extension(ExtensionValue { kind: &TIN_ELEMENT_TYPE, bytes: Arc::from(vec![VERSION, 0]) })
}

/// The kind of element type `name`.
fn kind_of(name: &str) -> Result<ElementKind> {
    ElementKind::from_name(name.trim())
        .ok_or_else(|| spatial(SqlState::SPATIAL_INVALID_ARGUMENT, "§17.1.2", &format!("'{name}' is no TIN element type")))
}

/// [§17.1.2 Description 2a, §17.1.6 Description 4c] That `g` is of the
/// geometry type the element type `kind` takes.
fn check(kind: ElementKind, g: &Geom) -> Result<()> {
    let (wanted, z) = takes(kind);
    if g.kind() != wanted || (z && !g.dims.has_z()) {
        let msg = format!("a {} element is of an {}{}", kind.name(), wanted.sql_name(), if z { " with z" } else { "" });
        return Err(spatial(SqlState::SPATIAL_INVALID_GEOMETRY, "§17.1.2 Description 2a", &msg));
    }
    Ok(())
}

/// An element's parts, as its payload holds them; none of each it has
/// not been given.
#[derive(Debug, Clone, Default)]
pub struct Parts {
    /// Its type's name.
    pub element_type: Option<String>,
    /// Its identifier.
    pub id: Option<i32>,
    /// Its tag.
    pub tag: Option<String>,
    /// Its geometry.
    pub geometry: Option<Geom>,
}

impl Parts {
    /// The parts of payload `bytes`, if they are one.
    fn of(bytes: &[u8]) -> Option<Parts> {
        let (&VERSION, rest) = bytes.split_first()? else { return None };
        let (&flags, mut rest) = rest.split_first()?;
        let id = if flags & HAS_ID == 0 {
            None
        } else {
            let (id, after) = rest.split_at_checked(ID_BYTES)?;
            rest = after;
            Some(i32::from_le_bytes(id.try_into().ok()?))
        };
        let mut text = |flag: u8| -> Option<Option<String>> {
            if flags & flag == 0 {
                return Some(None);
            }
            let (&n, after) = rest.split_first()?;
            let (s, after) = after.split_at_checked(usize::from(n))?;
            rest = after;
            String::from_utf8(s.to_vec()).ok().map(Some)
        };
        let (element_type, tag) = (text(HAS_TYPE)?, text(HAS_TAG)?);
        let geometry = if flags & HAS_GEOMETRY == 0 { rest.is_empty().then_some(None)? } else { Some(Geom::decode(rest).ok()?) };
        Some(Parts { element_type, id, tag, geometry })
    }

    /// The parts of `v`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if `v` is no ST_TINElement value.
    pub fn of_value(v: &Value) -> Result<Parts> {
        payload(v).and_then(Parts::of).ok_or_else(|| {
            SqlError::new(Code::SyntaxRule, "[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §17.1] expected an ST_TINElement value")
        })
    }

    /// [§17.1.7] Whether it is the empty set the constructor function
    /// returns: it has none of its parts.
    pub fn is_empty(&self) -> bool {
        self.element_type.is_none() && self.id.is_none() && self.tag.is_none() && self.geometry.is_none()
    }

    /// The element of the parts, checked: its type is one of §17.1.1
    /// Description 4, its tag no longer than 64 characters, and its
    /// geometry of the type its type takes.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF02 of an unknown type or a tag too long; 2FF74 of a
    /// geometry the type does not take.
    pub fn to_value(&self) -> Result<Value> {
        let kind = self.element_type.as_deref().map(kind_of).transpose()?;
        if let (Some(kind), Some(g)) = (kind, &self.geometry) {
            check(kind, g)?;
        }
        if self.tag.as_ref().is_some_and(|t| t.chars().count() > TAG_LENGTH) {
            return Err(spatial(SqlState::SPATIAL_INVALID_ARGUMENT, "§17.1.2", "the tag is longer than 64 characters"));
        }
        let flags = [
            (self.element_type.is_some(), HAS_TYPE),
            (self.id.is_some(), HAS_ID),
            (self.tag.is_some(), HAS_TAG),
            (self.geometry.is_some(), HAS_GEOMETRY),
        ]
        .into_iter()
        .fold(0_u8, |all, (has, flag)| if has { all | flag } else { all });
        let mut bytes = vec![VERSION, flags];
        if let Some(id) = self.id {
            bytes.extend_from_slice(&id.to_le_bytes());
        }
        let name = kind.map(ElementKind::name);
        for part in [name, self.tag.as_deref()].into_iter().flatten() {
            bytes.push(u8::try_from(part.len()).map_err(|_| spatial(SqlState::SPATIAL_INVALID_ARGUMENT, "§17.1.2", "a name too long"))?);
            bytes.extend_from_slice(part.as_bytes());
        }
        if let Some(g) = &self.geometry {
            bytes.extend_from_slice(&g.encode());
        }
        Ok(Value::Extension(ExtensionValue { kind: &TIN_ELEMENT_TYPE, bytes: bytes.into() }))
    }

    /// The parts of the model's element `e` of the TIN `tin`.
    pub fn of_model(e: &TinElement, tin: &Geom) -> Parts {
        let geometry = Geom { srid: tin.srid, dims: tin.dims, body: ModelBody::clone(&e.geometry) };
        Parts { element_type: Some(e.element_type.clone()), id: Some(e.id), tag: Some(e.tag.clone()), geometry: Some(geometry) }
    }

    /// The element as the well-known text of a TIN writes it, of an
    /// element with a type and a geometry.
    fn text(&self) -> String {
        let keyword = self.element_type.as_deref().and_then(ElementKind::from_name).map_or("", ElementKind::keyword);
        let geometry = self.geometry.as_ref().map(|g| wkt::write_geom(g).unwrap_or_default()).unwrap_or_default();
        format!("{keyword} ID {} TAG \"{}\" {geometry}", self.id.unwrap_or(0), self.tag.as_deref().unwrap_or_default())
    }
}

fn unkeyed() -> SqlError {
    SqlError::new(Code::SyntaxRule, "[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §17.1.1] ST_TINELEMENT has no ordering, so no key")
}

impl SqlType for TinElementType {
    fn name(&self) -> &'static str {
        ST_TINELEMENT
    }

    fn written(&self, _: &DataType) -> String {
        ST_TINELEMENT.to_owned()
    }

    fn comparable(&self, _: &DataType) -> bool {
        false
    }

    fn compare(&self, _: &Value, _: &Value) -> Option<Ordering> {
        None
    }

    fn bucket(&self, v: &Value) -> Option<Bucket> {
        Some(Bucket::Extension(payload(v)?.into()))
    }

    fn assign(&self, v: Value, _: &DataType, rule: &str) -> Result<Value> {
        if v.is_null() || payload(&v).is_some() {
            return Ok(v);
        }
        Err(SqlError::new(Code::Data, format!("[{rule}] {v} is not an ST_TINELEMENT value")))
    }

    fn cast(&self, v: Value, _: &DataType) -> Result<Value> {
        self.assign(v, &data_type(), "ISO/IEC 9075-2:2023 §6.13 SR8")
    }

    fn literal(&self, v: &Value) -> String {
        self.text(v)
    }

    fn text(&self, v: &Value) -> String {
        payload(v).and_then(Parts::of).map_or_else(String::new, |p| p.text())
    }

    fn position(&self, _: &Value) -> Option<f64> {
        None
    }

    fn descriptor(&self, _: &DataType) -> Descriptor {
        Descriptor::of(ST_TINELEMENT)
    }

    fn put_value(&self, v: &Value, out: &mut Vec<u8>) {
        out.put_bytes(payload(v).unwrap_or_default());
    }

    fn value(&self, r: &mut Reader<'_>) -> Result<Value> {
        let raw = r.bytes()?;
        if Parts::of(raw).is_none() {
            return Err(r.corrupt("an invalid ST_TINELEMENT value"));
        }
        Ok(Value::Extension(ExtensionValue { kind: &TIN_ELEMENT_TYPE, bytes: Arc::from(raw) }))
    }

    fn put_params(&self, _: &DataType, _: &mut Vec<u8>) {}

    fn params(&self, _: &mut Reader<'_>) -> Result<DataType> {
        Ok(data_type())
    }

    fn record_width(&self, _: &DataType) -> Option<usize> {
        None
    }

    fn encode_record(&self, v: &Value, _: &DataType) -> Result<Vec<u8>> {
        payload(v).map(<[u8]>::to_vec).ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] {v} is not an ST_TINELEMENT value")))
    }

    fn decode_record(&self, field: &[u8], _: &DataType) -> Option<Value> {
        Parts::of(field)?;
        Some(Value::Extension(ExtensionValue { kind: &TIN_ELEMENT_TYPE, bytes: Arc::from(field) }))
    }

    fn encode_key(&self, _: &Value, _: &DataType, _: &mut Vec<u8>) -> Result<()> {
        Err(unkeyed())
    }

    fn decode_key(&self, _: &mut &[u8], _: &DataType) -> Result<Value> {
        Err(unkeyed())
    }

    fn anchor(&self, v: &Value, _: &DataType) -> Option<(Value, bool)> {
        payload(v).map(|_| (v.clone(), true))
    }
}
