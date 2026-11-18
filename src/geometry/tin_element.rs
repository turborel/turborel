//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §17.1] The ST_TINElement type:
//! a TIN element's type, identifier, tag and geometry, which ST_TIN is
//! built from and ST_TINElements returns. Built in as a kind of type of
//! its own (RULES.md D53), its value a version octet, the identifier, the
//! type and the tag each after its length, and then the geometry's
//! payload. It has no ordering, so no key [§17.1.1].

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

/// [§17.1.2] The element of type `element_type`, identifier `id`, tag
/// `tag` and geometry `g`, checked.
///
/// # Errors
///
/// SQLSTATE 2FF02 of an unknown type or a tag longer than 64
/// characters; 2FF74 of a geometry the type does not take.
pub fn element(element_type: &str, id: i32, tag: &str, g: &Geom) -> Result<Value> {
    let kind = ElementKind::from_name(element_type.trim())
        .ok_or_else(|| spatial(SqlState::SPATIAL_INVALID_ARGUMENT, "§17.1.2", &format!("'{element_type}' is no TIN element type")))?;
    if tag.chars().count() > TAG_LENGTH {
        return Err(spatial(SqlState::SPATIAL_INVALID_ARGUMENT, "§17.1.2", "the tag is longer than 64 characters"));
    }
    let (wanted, z) = takes(kind);
    if g.kind() != wanted || (z && !g.dims.has_z()) {
        let msg = format!("a {} element is of an {}{}", kind.name(), wanted.sql_name(), if z { " with z" } else { "" });
        return Err(spatial(SqlState::SPATIAL_INVALID_GEOMETRY, "§17.1.2 Description 2a", &msg));
    }
    let name = kind.name().as_bytes();
    let mut bytes = vec![VERSION];
    bytes.extend_from_slice(&id.to_le_bytes());
    for part in [name, tag.as_bytes()] {
        bytes.push(u8::try_from(part.len()).map_err(|_| spatial(SqlState::SPATIAL_INVALID_ARGUMENT, "§17.1.2", "a name too long"))?);
        bytes.extend_from_slice(part);
    }
    bytes.extend_from_slice(&g.encode());
    Ok(Value::Extension(ExtensionValue { kind: &TIN_ELEMENT_TYPE, bytes: bytes.into() }))
}

/// An element's parts, as its payload holds them.
#[derive(Debug, Clone)]
pub struct Parts {
    /// Its type's name.
    pub element_type: String,
    /// Its identifier.
    pub id: i32,
    /// Its tag.
    pub tag: String,
    /// Its geometry.
    pub geometry: Geom,
}

impl Parts {
    /// The parts of payload `bytes`, if they are one.
    fn of(bytes: &[u8]) -> Option<Parts> {
        let (&VERSION, rest) = bytes.split_first()? else { return None };
        let id = i32::from_le_bytes(rest.get(..ID_BYTES)?.try_into().ok()?);
        let mut rest = rest.get(ID_BYTES..)?;
        let mut text = || -> Option<String> {
            let (&n, after) = rest.split_first()?;
            let (s, after) = after.split_at_checked(usize::from(n))?;
            rest = after;
            String::from_utf8(s.to_vec()).ok()
        };
        let (element_type, tag) = (text()?, text()?);
        let geometry = Geom::decode(rest).ok()?;
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

    /// The element of the parts.
    ///
    /// # Errors
    ///
    /// As `element`.
    pub fn to_value(&self) -> Result<Value> {
        element(&self.element_type, self.id, &self.tag, &self.geometry)
    }

    /// The parts of the model's element `e` of the TIN `tin`.
    pub fn of_model(e: &TinElement, tin: &Geom) -> Parts {
        let geometry = Geom { srid: tin.srid, dims: tin.dims, body: ModelBody::clone(&e.geometry) };
        Parts { element_type: e.element_type.clone(), id: e.id, tag: e.tag.clone(), geometry }
    }

    /// The element as the well-known text of a TIN writes it.
    fn text(&self) -> String {
        let keyword = ElementKind::from_name(&self.element_type).map_or("", ElementKind::keyword);
        let geometry = wkt::write_geom(&self.geometry).unwrap_or_default();
        format!("{keyword} ID {} TAG \"{}\" {geometry}", self.id, self.tag)
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
