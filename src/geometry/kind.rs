//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.2] The 27 geometry types,
//! built in, each a kind of type through the SqlType contract (RULES.md
//! D53) and one family (`value/types/family.rs`, RULES.md
//! D120): ST_GEOMETRY, which GEOMETRY names too, and its
//! subtypes. Every value is of one payload (`model/payload.rs`), whose
//! type code is its most specific type; a value of a type is a value of
//! each supertype, and is assigned to a site of a supertype as it is.
//!
//! Values compare as their payloads' bytes here, for the keys and
//! buckets the engine keeps of them; SQL compares them by
//! ST_OrderingEquals [§5.1.65], EQUALS ONLY BY RELATIVE, which the binder
//! calls (`binder/structured/orderings.rs`).

mod types;

use super::model::{self, ALL, Geom, KINDS, Kind};
use super::{Geometry, invalid};
use crate::error::{Code, Result, SqlError};
use crate::value::types::family::Family;
use crate::value::types::{ExtensionType, ExtensionValue, Kind as TypeKind, TypeParams};
use crate::value::{DataType, SqlType, Value};

/// The name GEOMETRY, which names ST_GEOMETRY too [ISO/IEC 19125-2].
pub const NAME: &str = "GEOMETRY";
/// The bytes of a payload before its type code: the version, the SRID
/// and the byte order.
const CODE_AT: usize = 6;
/// The bytes of a type code.
const CODE_BYTES: usize = 4;
/// [§5.1.68 Table 15] The codes of z, m and both, past the base code.
const DIMENSION_CODES: u32 = 1000;
/// The codes of the seven kinds of x and y D85 reads itself.
const BASIC: std::ops::RangeInclusive<u32> = 1..=7;

/// A geometry type: one of the 27 of the hierarchy, as a kind of type.
#[derive(Debug)]
pub struct GeometryType {
    kind: Kind,
}

/// The types, as `model::ALL` orders them: ST_GEOMETRY first.
pub static TYPES: [GeometryType; KINDS] = [
    GeometryType { kind: Kind::Geometry },
    GeometryType { kind: Kind::Point },
    GeometryType { kind: Kind::Curve },
    GeometryType { kind: Kind::LineString },
    GeometryType { kind: Kind::CircularString },
    GeometryType { kind: Kind::Circle },
    GeometryType { kind: Kind::GeodesicString },
    GeometryType { kind: Kind::EllipticalCurve },
    GeometryType { kind: Kind::NurbsCurve },
    GeometryType { kind: Kind::Clothoid },
    GeometryType { kind: Kind::SpiralCurve },
    GeometryType { kind: Kind::CompoundCurve },
    GeometryType { kind: Kind::Surface },
    GeometryType { kind: Kind::CurvePolygon },
    GeometryType { kind: Kind::Polygon },
    GeometryType { kind: Kind::Triangle },
    GeometryType { kind: Kind::PolyhdrlSurface },
    GeometryType { kind: Kind::Tin },
    GeometryType { kind: Kind::CompoundSurface },
    GeometryType { kind: Kind::Solid },
    GeometryType { kind: Kind::BRepSolid },
    GeometryType { kind: Kind::GeomCollection },
    GeometryType { kind: Kind::MultiPoint },
    GeometryType { kind: Kind::MultiCurve },
    GeometryType { kind: Kind::MultiLineString },
    GeometryType { kind: Kind::MultiSurface },
    GeometryType { kind: Kind::MultiPolygon },
];

/// The types as kinds of type.
static REFS: [&dyn SqlType; KINDS] = [
    &TYPES[0], &TYPES[1], &TYPES[2], &TYPES[3], &TYPES[4], &TYPES[5], &TYPES[6], &TYPES[7], &TYPES[8], &TYPES[9], &TYPES[10], &TYPES[11],
    &TYPES[12], &TYPES[13], &TYPES[14], &TYPES[15], &TYPES[16], &TYPES[17], &TYPES[18], &TYPES[19], &TYPES[20], &TYPES[21], &TYPES[22],
    &TYPES[23], &TYPES[24], &TYPES[25], &TYPES[26],
];

/// The types as a type holds them.
static KIND_OF: [TypeKind; KINDS] = [
    &REFS[0], &REFS[1], &REFS[2], &REFS[3], &REFS[4], &REFS[5], &REFS[6], &REFS[7], &REFS[8], &REFS[9], &REFS[10], &REFS[11], &REFS[12],
    &REFS[13], &REFS[14], &REFS[15], &REFS[16], &REFS[17], &REFS[18], &REFS[19], &REFS[20], &REFS[21], &REFS[22], &REFS[23], &REFS[24],
    &REFS[25], &REFS[26],
];

/// ST_GEOMETRY, the root of the family, the kind every value carries.
pub static GEOMETRY: &dyn SqlType = &TYPES[0];

/// The family of the geometry types.
#[derive(Debug)]
pub struct Geometries;

/// The family.
pub(super) static FAMILY: Geometries = Geometries;

impl GeometryType {
    /// Its geometry type.
    pub fn kind(&self) -> Kind {
        self.kind
    }
}

/// The kinds of type of the family, ST_GEOMETRY first.
pub fn kinds() -> &'static [TypeKind] {
    &KIND_OF
}

/// The kind of type of geometry type `kind`.
pub fn kind_of(kind: Kind) -> TypeKind {
    let at = ALL.iter().position(|k| *k == kind).unwrap_or_default();
    KIND_OF.get(at).unwrap_or(&KIND_OF[0])
}

/// The data type of geometry type `kind`, of no SRID.
pub fn data_type_of(kind: Kind) -> DataType {
    DataType::Extension(ExtensionType { kind: kind_of(kind), params: TypeParams::default() })
}

/// The data type ST_GEOMETRY, of no SRID.
pub fn data_type() -> DataType {
    data_type_of(Kind::Geometry)
}

/// The geometry type of the kind of type `t`, if it is one.
pub fn geometry_kind(t: &dyn SqlType) -> Option<Kind> {
    is_geometry(t).then(|| Kind::named(t.name())).flatten()
}

/// Whether the type `t` is a geometry type, of this family: by the
/// family alone, so a row at a time costs no search of the kinds' names.
pub fn is_geometry(t: &dyn SqlType) -> bool {
    let family: &dyn Family = &FAMILY;
    t.family().is_some_and(|f| std::ptr::addr_eq(f, family))
}

/// The geometry type of the data type `t`, if it is one.
pub fn of_data_type(t: &DataType) -> Option<Kind> {
    let DataType::Extension(e) = t else { return None };
    geometry_kind(*e.kind)
}

/// The type code of payload `bytes`.
fn code(bytes: &[u8]) -> Option<u32> {
    let code: [u8; CODE_BYTES] = bytes.get(CODE_AT..CODE_AT.checked_add(CODE_BYTES)?)?.try_into().ok()?;
    Some(u32::from_le_bytes(code))
}

/// The most specific type of payload `bytes`, as its type code says.
pub fn most_specific(bytes: &[u8]) -> Option<Kind> {
    let code = code(bytes)?;
    let base = if code > model::WKB_DEPRECATED_BASE { code } else { code.checked_rem(DIMENSION_CODES)? };
    Kind::of_wkb_code(base)
}

/// A geometry value of payload `bytes`, which are a payload's.
pub(crate) fn wrap(bytes: Vec<u8>) -> Value {
    Value::Extension(ExtensionValue { kind: GEOMETRY, bytes: bytes.into() })
}

/// The payload of a geometry value `v`: of a value of this family, by
/// the family alone, so a row at a time costs no search of the kinds.
pub(crate) fn payload(v: &Value) -> Option<&[u8]> {
    let Value::Extension(e) = v else { return None };
    is_geometry(e.kind).then(|| e.bytes.as_ref())
}

pub(super) fn value(g: &Geometry) -> Value {
    let mut bytes = vec![model::PAYLOAD_VERSION];
    bytes.extend_from_slice(&g.srid().to_le_bytes());
    bytes.extend_from_slice(&g.to_wkb());
    wrap(bytes)
}

pub(super) fn geometry(v: &Value) -> Result<Geometry> {
    decode(payload(v).ok_or_else(|| invalid("expected a geometry value"))?)
}

/// The planar form of a payload: as D85 reads one of the seven kinds of
/// x and y; else, or of one holding a part of another kind, the linear
/// form of x and y of its model value (`Geom::linear`, `Geom::xy`).
pub(super) fn decode(bytes: &[u8]) -> Result<Geometry> {
    if code(bytes).is_some_and(|c| BASIC.contains(&c)) {
        let (srid, wkb) = model::header(bytes)?;
        let read = Geometry::from_canonical_wkb(wkb, srid);
        if let Err(e) = &read
            && e.code != Code::Unsupported
        {
            return Err(SqlError::new(Code::Corrupt, "[DESIGN] an invalid or noncanonical stored geometry"));
        }
        if let Ok(g) = read {
            return Ok(g);
        }
    }
    Geometry::from_geom(&Geom::decode(bytes)?.linear()?.xy())
}

/// That payload `bytes` are a stored value's.
pub(super) fn check(bytes: &[u8]) -> Result<()> {
    if code(bytes).is_some_and(|c| BASIC.contains(&c)) {
        let (srid, wkb) = model::header(bytes)?;
        if Geometry::from_canonical_wkb(wkb, srid).is_ok() {
            return Ok(());
        }
    }
    Geom::decode(bytes).map(drop)
}
