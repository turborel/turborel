//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.2.2 to §7.5.2, §7.10.2,
//! §8.2.2 to §8.5.2, §10.1.2 to §10.6.2] The constructors defined as the
//! empty value given an SRID and then mutators: a curve's points, a
//! compound curve's curves, a surface's rings, a polyhedral surface's
//! patches and a collection's elements, `SELF.ST_SRID(ansrid)` and then
//! ST_Points, ST_Curves, ST_ExteriorRing and ST_InteriorRings,
//! ST_Patches or ST_Geometries. A circular string and a circle given by
//! a center are built in `arcs.rs`. A collection of one geometry or of
//! an array without an SRID takes theirs [§10.1.2 Description 10, 12].

use super::Param;
use crate::error::Result;
use crate::functions::ScalarFunction;
use crate::geometry::collections::CollectionFunction;
use crate::geometry::model::{Dims, Geom, Kind, empty_body};
use crate::geometry::mutators::{Mutator, lists, rings};
use crate::geometry::observers::{signature, whole};
use crate::geometry::support::checked_geometries;
use crate::value::Value;

/// The SRID `v`, an INTEGER.
pub(super) fn srid(v: Option<&Value>, name: &str) -> Result<i32> {
    i32::try_from(whole(v, name)?).map_err(|_| signature(name))
}

/// The empty value of `kind` in the system `srid`.
fn empty(kind: Kind, srid: i32) -> Result<Geom> {
    Geom::new(srid, Dims::Xy, empty_body(kind).ok_or_else(|| signature(kind.type_name()))?)
}

/// An ARRAY of `v` alone, or `v` if it is one.
fn as_array(v: &Value) -> Value {
    if let Value::Array(_) = v { v.clone() } else { Value::Array(vec![v.clone()].into()) }
}

/// [§10.1.2 Description 10, 12] The SRID of a collection given no SRID:
/// its geometry's, or its array's one, 0 of none.
fn collection_srid(v: &Value) -> Result<i32> {
    if let Value::Array(_) = v {
        return checked_geometries(v, false).map(|(_, srid)| srid);
    }
    Ok(Geom::from_value(v)?.srid)
}

/// [§8.2.2, §8.3.2, §8.4.2] A surface of the ring `exterior` and the
/// rings `interiors`, none if not given.
fn surface(g: &Geom, exterior: &Value, interiors: Option<&Value>) -> Result<Geom> {
    let none = Value::Array(Vec::new().into());
    rings::interior(&rings::exterior(g, exterior)?, interiors.unwrap_or(&none))
}

/// The value of `kind` the form `form` of `args` builds, none null.
pub(super) fn build(kind: Kind, form: &[Param], args: &[Value]) -> Result<Value> {
    let name = kind.type_name();
    let with_srid = form.len() > 1 && form.last() == Some(&Param::Integer);
    let (given, srid) = match (with_srid, args.split_last()) {
        (true, Some((last, rest))) => (rest, srid(Some(last), name)?),
        (false, Some(_) | None) | (true, None) => (args, 0_i32),
    };
    let [first, rest @ ..] = given else { return Err(signature(name)) };
    if form.first() == Some(&Param::One(Kind::Point)) || form.get(1) == Some(&Param::Doubles) {
        return super::arcs::build(kind, given, srid);
    }
    let collection = matches!(kind, Kind::GeomCollection) && given.len() == args.len();
    let g = empty(kind, if collection { collection_srid(first)? } else { srid })?;
    let made = match kind {
        Kind::LineString | Kind::GeodesicString | Kind::CircularString | Kind::Circle => lists::replace(Mutator::Points, &g, first)?,
        Kind::CompoundCurve => lists::replace(Mutator::Curves, &g, &as_array(first))?,
        Kind::CurvePolygon | Kind::Polygon => surface(&g, first, rest.first())?,
        Kind::Triangle => {
            let ring = if let Value::Array(_) = first {
                lists::replace(Mutator::Points, &empty(Kind::LineString, srid)?, first)?.to_value()
            } else {
                first.clone()
            };
            surface(&g, &ring, None)?
        }
        Kind::PolyhdrlSurface => rings::patches(&g, first)?,
        Kind::GeomCollection | Kind::MultiPoint | Kind::MultiCurve | Kind::MultiLineString | Kind::MultiSurface | Kind::MultiPolygon => {
            return CollectionFunction::Geometries(kind).call(&[g.to_value(), as_array(first)]);
        }
        Kind::Point
        | Kind::Geometry
        | Kind::Curve
        | Kind::EllipticalCurve
        | Kind::NurbsCurve
        | Kind::Clothoid
        | Kind::SpiralCurve
        | Kind::Surface
        | Kind::Tin
        | Kind::CompoundSurface
        | Kind::Solid
        | Kind::BRepSolid => return Err(signature(name)),
    };
    Ok(made.to_value())
}
