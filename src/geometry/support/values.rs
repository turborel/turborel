//! The stored form of each type's values: the bytes of an extension value
//! (`Value::Extension`), read back by the same type. An attribute the
//! standard lets be the null value is written with a byte saying whether
//! it is present, so a value made by the constructor function, every
//! attribute null, is the empty set its clause describes [§17.1.1
//! Description 2, §17.2.1 Description 5, §17.3.1 Description 4, §17.4.1
//! Description 4, §17.5.1 Description 4].
//!
//! - ST_Angle and ST_Direction are their radians, eight octets, negative
//!   zero written as zero, so equal values have equal bytes.
//! - ST_SpatialRefSys is its SRID, if it has one, and its canonical
//!   `<spatial reference system>`, empty for the unspecified plane.
//! - ST_Vector is its SRID and its x, y and z.
//! - A geometry attribute is the payload of its geometry value.

use super::{Support, of_value, wrap};
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError};
use crate::spatial::srs::angle::Angle;
use crate::spatial::srs::direction::Direction;
use crate::spatial::srs::system::SpatialRefSys;
use crate::value::Value;

/// SQLCODE -301: `v` is not a value of `s`.
pub(super) fn not_of(s: Support) -> SqlError {
    SqlError::new(Code::Data, format!("[{} {}] expected a value of {}", s.standard(), s.clause(), s.sql_name()))
}

/// The bytes of `v`, a value of `s`.
pub(super) fn bytes_of(v: &Value, s: Support) -> Result<&[u8]> {
    of_value(v).filter(|(of, _)| *of == s).map(|(_, b)| b).ok_or_else(|| not_of(s))
}

/// Writes an attribute that may be absent.
pub(super) fn put_some<T>(out: &mut Vec<u8>, v: Option<T>, put: impl FnOnce(&mut Vec<u8>, T)) {
    out.put_u8(u8::from(v.is_some()));
    if let Some(v) = v {
        put(out, v);
    }
}

/// Reads an attribute `put_some` wrote.
pub(super) fn some<'a, T>(r: &mut Reader<'a>, read: impl FnOnce(&mut Reader<'a>) -> Result<T>) -> Result<Option<T>> {
    match r.u8()? {
        0 => Ok(None),
        1 => read(r).map(Some),
        _ => Err(r.corrupt("a presence byte is neither 0 nor 1")),
    }
}

fn put_f64(out: &mut Vec<u8>, v: f64) {
    out.put_u64((v + 0.0_f64).to_bits());
}

fn f64_of(r: &mut Reader<'_>) -> Result<f64> {
    let v = f64::from_bits(r.u64()?);
    if v.is_finite() { Ok(v) } else { Err(r.corrupt("a number is not finite")) }
}

fn put_i32(out: &mut Vec<u8>, v: i32) {
    out.put_u32(v.cast_unsigned());
}

fn i32_of(r: &mut Reader<'_>) -> Result<i32> {
    Ok(r.u32()?.cast_signed())
}

/// Writes a geometry value's payload.
pub(super) fn put_geometry(out: &mut Vec<u8>, g: &[u8]) {
    out.put_bytes(g);
}

/// Reads a geometry payload `put_geometry` wrote, checked as stored.
pub(super) fn geometry_of(r: &mut Reader<'_>) -> Result<Vec<u8>> {
    let bytes = r.bytes()?;
    super::super::kind::check(bytes).map_err(|_| r.corrupt("an invalid geometry attribute"))?;
    Ok(bytes.to_vec())
}

/// The bytes of a value, read whole by `read`.
pub(super) fn read_all<T>(bytes: &[u8], s: Support, read: impl FnOnce(&mut Reader<'_>) -> Result<T>) -> Result<T> {
    let mut r = Reader::new(bytes, s.sql_name());
    let v = read(&mut r)?;
    r.finish()?;
    Ok(v)
}

/// A number, finite, that is a value's only attribute.
fn number(bytes: &[u8], s: Support) -> Result<f64> {
    read_all(bytes, s, f64_of)
}

/// An ST_Angle value.
pub(super) fn angle_value(a: Angle) -> Value {
    let mut out = Vec::new();
    put_f64(&mut out, a.radians());
    wrap(Support::Angle, out)
}

/// The angle `v` holds.
pub(super) fn angle_of(v: &Value) -> Result<Angle> {
    number(bytes_of(v, Support::Angle)?, Support::Angle).map(Angle::new)
}

/// An ST_Direction value.
pub(super) fn direction_value(d: Direction) -> Value {
    let mut out = Vec::new();
    put_f64(&mut out, d.radians());
    wrap(Support::Direction, out)
}

/// The direction `v` holds.
pub(super) fn direction_of(v: &Value) -> Result<Direction> {
    let radians = number(bytes_of(v, Support::Direction)?, Support::Direction)?;
    Direction::new(radians).map_err(|f| super::super::routines::fault(&f))
}

/// The radians of an angle's or a direction's bytes, which order their
/// values (`types.rs`).
pub(crate) fn direction_radians(bytes: &[u8]) -> Option<f64> {
    Some(f64::from_bits(u64::from_le_bytes(bytes.try_into().ok()?)))
}

/// An ST_SpatialRefSys value.
pub(super) fn system_value(s: &SpatialRefSys) -> Value {
    let mut out = Vec::new();
    put_some(&mut out, s.srid(), put_i32);
    out.put_str(&s.as_wkt().unwrap_or_default());
    wrap(Support::SpatialRefSys, out)
}

/// The system `v` holds.
pub(super) fn system_of(v: &Value) -> Result<SpatialRefSys> {
    read_all(bytes_of(v, Support::SpatialRefSys)?, Support::SpatialRefSys, |r| {
        let srid = some(r, i32_of)?;
        let text = r.str()?;
        let system = if text.is_empty() {
            SpatialRefSys::unspecified()
        } else {
            SpatialRefSys::from_text(&text).map_err(|_| r.corrupt("an unreadable definition"))?
        };
        Ok(match srid {
            Some(id) => system.with_srid(id),
            None => system,
        })
    })
}

/// [§17.2] An ST_Vector value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Vector {
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub z: Option<f64>,
    pub srid: i32,
}

impl Vector {
    /// The empty set, of SRID 0 [§17.2.1 Description 5].
    pub const EMPTY: Vector = Vector { x: None, y: None, z: None, srid: 0 };

    fn put(&self, out: &mut Vec<u8>) {
        put_i32(out, self.srid);
        for c in [self.x, self.y, self.z] {
            put_some(out, c, put_f64);
        }
    }

    fn read(r: &mut Reader<'_>) -> Result<Vector> {
        let srid = i32_of(r)?;
        Ok(Vector { srid, x: some(r, f64_of)?, y: some(r, f64_of)?, z: some(r, f64_of)? })
    }

    /// Its value.
    pub fn value(&self) -> Value {
        wrap(Support::Vector, vector_bytes(self))
    }

    /// The vector `v` holds.
    pub fn of(v: &Value) -> Result<Vector> {
        read_all(bytes_of(v, Support::Vector)?, Support::Vector, Vector::read)
    }

    /// [§17.2.9] Whether it is the empty set: it has no x and no y.
    pub fn is_empty(&self) -> bool {
        self.x.is_none() && self.y.is_none()
    }

    /// [§17.2.10] ST_Equals.
    pub fn equals(&self, other: &Vector) -> bool {
        let bits = |c: Option<f64>| c.map(|n| (n + 0.0_f64).to_bits());
        self.srid == other.srid && [(self.x, other.x), (self.y, other.y), (self.z, other.z)].into_iter().all(|(a, b)| bits(a) == bits(b))
    }
}

/// The bytes of vector `v`.
pub(crate) fn vector_bytes(v: &Vector) -> Vec<u8> {
    let mut out = Vec::new();
    v.put(&mut out);
    out
}

/// [§17.5] An ST_Knot value.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub(super) struct Knot {
    pub value: Option<f64>,
    pub multiplicity: Option<i32>,
}

impl Knot {
    pub fn value(&self) -> Value {
        let mut out = Vec::new();
        put_some(&mut out, self.value, put_f64);
        put_some(&mut out, self.multiplicity, put_i32);
        wrap(Support::Knot, out)
    }

    pub fn of(v: &Value) -> Result<Knot> {
        read_all(bytes_of(v, Support::Knot)?, Support::Knot, |r| Ok(Knot { value: some(r, f64_of)?, multiplicity: some(r, i32_of)? }))
    }
}

/// [§17.4] An ST_NURBSPoint value: its weighted point's payload and its
/// weight.
#[derive(Clone, Debug, PartialEq, Default)]
pub(super) struct NurbsPoint {
    pub point: Option<Vec<u8>>,
    pub weight: Option<f64>,
}

impl NurbsPoint {
    pub fn value(&self) -> Value {
        let mut out = Vec::new();
        put_some(&mut out, self.point.as_deref(), put_geometry);
        put_some(&mut out, self.weight, put_f64);
        wrap(Support::NurbsPoint, out)
    }

    pub fn of(v: &Value) -> Result<NurbsPoint> {
        read_all(bytes_of(v, Support::NurbsPoint)?, Support::NurbsPoint, |r| {
            Ok(NurbsPoint { point: some(r, geometry_of)?, weight: some(r, f64_of)? })
        })
    }
}

/// [§17.3] An ST_AffinePlacement value: its location's payload and its
/// reference directions.
#[derive(Clone, Debug, PartialEq, Default)]
pub(super) struct Placement {
    pub location: Option<Vec<u8>>,
    pub directions: Option<Vec<Vector>>,
}

impl Placement {
    pub fn value(&self) -> Value {
        let mut out = Vec::new();
        put_some(&mut out, self.location.as_deref(), put_geometry);
        put_some(&mut out, self.directions.as_deref(), |o, all| {
            o.put_u32(u32::try_from(all.len()).unwrap_or(u32::MAX));
            for v in all {
                v.put(o);
            }
        });
        wrap(Support::AffinePlacement, out)
    }

    pub fn of(v: &Value) -> Result<Placement> {
        read_all(bytes_of(v, Support::AffinePlacement)?, Support::AffinePlacement, |r| {
            let location = some(r, geometry_of)?;
            let directions = some(r, |r| {
                let n = r.u32()?;
                (0..n).map(|_| Vector::read(r)).collect::<Result<Vec<_>>>()
            })?;
            Ok(Placement { location, directions })
        })
    }
}

/// That `bytes` are a stored value of `s`.
pub(super) fn check(s: Support, bytes: &[u8]) -> Result<()> {
    let v = wrap(s, bytes.to_vec());
    match s {
        Support::SpatialRefSys => system_of(&v).map(drop),
        Support::Angle => angle_of(&v).map(drop),
        Support::Direction => direction_of(&v).map(drop),
        Support::Vector => Vector::of(&v).map(drop),
        Support::AffinePlacement => Placement::of(&v).map(drop),
        Support::NurbsPoint => NurbsPoint::of(&v).map(drop),
        Support::Knot => Knot::of(&v).map(drop),
        Support::TextElement | Support::TextElementArray | Support::AnnotationText => super::annotation::check(s, &v),
    }
}

/// [§14.1.7, §17.2.20] ST_OrderingEquals of two values of a type ordered
/// EQUALS ONLY: 0 if ST_Equals holds of them, else 1; none if they are
/// not two values of one such type.
pub(crate) fn ordering_equals(a: &Value, b: &Value) -> Option<Result<bool>> {
    let ((s, _), (t, _)) = (of_value(a)?, of_value(b)?);
    if s != t {
        return None;
    }
    match s {
        Support::SpatialRefSys => Some(system_of(a).and_then(|x| system_of(b).map(|y| x.equals(&y)))),
        Support::Vector => Some(Vector::of(a).and_then(|x| Vector::of(b).map(|y| x.equals(&y)))),
        Support::Angle
        | Support::Direction
        | Support::AffinePlacement
        | Support::NurbsPoint
        | Support::Knot
        | Support::TextElement
        | Support::TextElementArray
        | Support::AnnotationText => None,
    }
}
