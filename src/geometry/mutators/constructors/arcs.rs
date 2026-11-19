//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.3.2 Description 14 to 22,
//! §7.4.2] The constructors of a circular string of control points with
//! bulges and bulge normals, or of a center, a radius and two angles, and
//! of a circle of a center, a radius and a normal. Each derives the
//! points the curve takes (`spatial::curves::circular`), points of the
//! given ones' spatial reference system and dimensions, and is then
//! `SELF.ST_SRID(ansrid).ST_Points(points)`, as checked as ST_Points
//! checks them.

use crate::error::Result;
use crate::error::SqlState;
use crate::geometry::model::{Body, Dims, Geom, Kind, empty_body};
use crate::geometry::mutators::{Mutator, invalid, lists};
use crate::geometry::observers::signature;
use crate::geometry::representations::spatial;
use crate::geometry::support::{self, checked_geometries};
use crate::spatial::curves::circular;
use crate::spatial::curves::numeric::V3;
use crate::spatial::curves::types::Coord;
use crate::value::Value;

const CLAUSE: &str = "§7.3.2 Description 16, 21";

/// The point of `coords` of `dims` as the curve engines take it.
fn coord(coords: &[f64], dims: Dims) -> Coord {
    let at = |k: usize| coords.get(k).copied().unwrap_or_default();
    Coord { x: at(0), y: at(1), z: dims.z_at().map_or(0.0_f64, at), m: dims.m_at().map_or(0.0_f64, at) }
}

/// The ST_Point values of `points` of `dims` in the system `srid`.
fn values(points: &[Coord], dims: Dims, srid: i32) -> Result<Value> {
    let one = |p: &Coord| -> Result<Value> {
        let coords = [Some(p.x), Some(p.y), dims.has_z().then_some(p.z), dims.has_m().then_some(p.m)].into_iter().flatten().collect();
        Ok(Geom::new(srid, dims, Body::Point(coords))?.to_value())
    };
    Ok(Value::Array(points.iter().map(one).collect::<Result<Vec<_>>>()?.into()))
}

/// The point `v`, not empty.
fn center(v: &Value) -> Result<(Coord, Dims, i32)> {
    let g = Geom::from_value(v)?;
    let Body::Point(coords) = &g.body else { return Err(signature("ST_CIRCULARSTRING")) };
    if coords.is_empty() {
        return Err(spatial(SqlState::SPATIAL_EMPTY_POINT_VALUE, CLAUSE, "empty point value"));
    }
    Ok((coord(coords, g.dims), g.dims, g.srid))
}

/// The DOUBLE PRECISION `v`.
fn number(v: &Value) -> Result<f64> {
    if let Value::Approx(x) = v { Ok(*x) } else { Err(signature("ST_CIRCULARSTRING")) }
}

/// The vector `v` as the engines take it, z 0 if it has none.
fn vector(v: &Value) -> Result<V3> {
    let parts = support::vector_parts(v)?.ok_or_else(|| invalid(CLAUSE, "a normal is not empty"))?;
    Ok([parts.x, parts.y, parts.z.unwrap_or_default()])
}

/// The elements of the ARRAY `v`, none null.
fn elements(v: &Value) -> Result<&[Value]> {
    let Value::Array(items) = v else { return Err(signature("ST_CIRCULARSTRING")) };
    if items.iter().any(Value::is_null) {
        return Err(spatial(SqlState::SPATIAL_ELEMENT_IS_A_NULL_VALUE, CLAUSE, "element is a null value"));
    }
    Ok(items)
}

/// [§7.3.2 Description 16] The points of control points with bulges and
/// bulge normals.
fn of_bulges(points: &Value, bulges: &Value, normals: &Value) -> Result<(Vec<Coord>, Dims, i32)> {
    let (all, srid) = checked_geometries(points, true)?;
    let dims = all.first().map_or(Dims::Xy, |p| p.dims);
    let mut controls = Vec::new();
    for p in &all {
        let Body::Point(coords) = &p.body else { return Err(signature("ST_CIRCULARSTRING")) };
        controls.push(coord(coords, dims));
    }
    let bulges: Vec<f64> = elements(bulges)?.iter().map(number).collect::<Result<_>>()?;
    let normals: Vec<V3> = elements(normals)?.iter().map(vector).collect::<Result<_>>()?;
    Ok((circular::from_bulges(&controls, &bulges, &normals)?, dims, srid))
}

/// [§7.3.2, §7.4.2] The points of a circular string or a circle of the
/// arguments `given`, their dimensions and system.
fn derived(kind: Kind, given: &[Value]) -> Result<(Vec<Coord>, Dims, i32)> {
    if let [points @ Value::Array(_), bulges, normals] = given {
        return of_bulges(points, bulges, normals);
    }
    match given {
        [at, radius, start, end] => {
            let (c, dims, of) = center(at)?;
            let points = circular::from_center(c, number(radius)?, support::angle_radians(start)?, support::angle_radians(end)?)?;
            Ok((points, dims, of))
        }
        [at, radius, normal] => {
            let (c, dims, of) = center(at)?;
            Ok((circular::circle_from_center(c, number(radius)?, vector(normal)?, dims.has_z())?, dims, of))
        }
        [..] => Err(signature(kind.type_name())),
    }
}

/// The circular string or circle the arguments `given` give, in the
/// system `srid`.
pub(super) fn build(kind: Kind, given: &[Value], srid: i32) -> Result<Value> {
    let (points, dims, of) = derived(kind, given)?;
    let empty = Geom::new(srid, Dims::Xy, empty_body(kind).ok_or_else(|| signature(kind.type_name()))?)?;
    Ok(lists::replace(Mutator::Points, &empty, &values(&points, dims, of)?)?.to_value())
}
