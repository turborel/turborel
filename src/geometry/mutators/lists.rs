//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.2.3, §7.3.3, §7.4.3,
//! §7.5.3, §7.10.3, §8.4.3] The mutators of a curve's or a triangle's
//! points and of a compound curve's curves: ST_Points(ST_Point ARRAY) and
//! ST_Curves(ST_Curve ARRAY). Each checks the array as ST_CheckNulls and,
//! of points, ST_CheckConsecDups do; an array that is not empty is of
//! SELF's spatial reference system, and the points are of one coordinate
//! dimension, the value's.

use super::{Mutator, invalid, mixed_systems};
use crate::error::{Result, SqlState};
use crate::geometry::model::{Body, Curve, Dims, Geom, Surface};
use crate::geometry::observers::signature;
use crate::geometry::representations::spatial;
use crate::geometry::support::{checked_geometries, same_dims};
use crate::spatial::curves::circular;
use crate::spatial::curves::types::Coord;
use crate::value::Value;

/// The points of a triangle's ring [§8.4.3 Description 4b].
const TRIANGLE_POINTS: usize = 4;
/// The points of a circle [§7.4.3 Description 4b].
const CIRCLE_POINTS: usize = 3;

/// The geometries of `array` in `g`'s system, and their dimensions, `g`'s
/// of none.
fn elements(m: Mutator, g: &Geom, array: &Value, consecutive: bool) -> Result<(Vec<Geom>, Dims)> {
    let (all, srid) = checked_geometries(array, consecutive)?;
    if !all.is_empty() && srid != g.srid {
        return Err(mixed_systems(clause(m, g)));
    }
    same_dims(&all)?;
    let dims = all.first().map_or(g.dims, |first| first.dims);
    Ok((all, dims))
}

/// The clause of the mutator `m` of `g`.
fn clause(m: Mutator, g: &Geom) -> &'static str {
    match (m, &g.body) {
        (Mutator::Curves, _) => "§7.10.3",
        (_, Body::Curve(Curve::CircularString(_))) => "§7.3.3",
        (_, Body::Curve(Curve::Circle(_))) => "§7.4.3",
        (_, Body::Curve(Curve::GeodesicString(_))) => "§7.5.3",
        (_, Body::Surface(_)) => "§8.4.3",
        (_, Body::Point(_) | Body::Curve(_) | Body::Solid(_) | Body::Collection(_)) => "§7.2.3",
    }
}

/// The coordinates of `points`, each a point.
fn coordinates(m: Mutator, points: &[Geom]) -> Result<Vec<f64>> {
    let mut out = Vec::new();
    for p in points {
        let Body::Point(c) = &p.body else { return Err(signature(m.name())) };
        out.extend_from_slice(c);
    }
    Ok(out)
}

/// The point at `i` of coordinates of `dims` as the curve engines take
/// it.
fn coord(coords: &[f64], dims: Dims, i: usize) -> Coord {
    let stride = dims.stride();
    let at = |k: usize| coords.get(i.saturating_mul(stride).saturating_add(k)).copied().unwrap_or_default();
    Coord { x: at(0), y: at(1), z: dims.z_at().map_or(0.0_f64, at), m: dims.m_at().map_or(0.0_f64, at) }
}

/// [§7.3.3 Description 4c, §7.4.3 Description 4c, 4d] The conditions on
/// the points of a circular string or a circle.
fn arcs(g: &Geom, coords: &[f64], dims: Dims) -> Result<()> {
    let count = coords.len().checked_div(dims.stride()).unwrap_or_default();
    let same = |a: usize, b: usize| {
        let (p, q) = (coord(coords, dims, a), coord(coords, dims, b));
        let bits = |c: Coord| [c.x, c.y, c.z].map(|v| (v + 0.0_f64).to_bits());
        bits(p) == bits(q)
    };
    let duplicate = || spatial(SqlState::SPATIAL_DUPLICATE_VALUE, clause(Mutator::Points, g), "duplicate value");
    if let Body::Curve(Curve::Circle(_)) = &g.body {
        if count != CIRCLE_POINTS {
            return Err(spatial(SqlState::SPATIAL_EXACTLY_THREE_POINTS, "§7.4.3 Description 4b", "exactly three points"));
        }
        if same(0, CIRCLE_POINTS - 1) {
            return Err(duplicate());
        }
        let three = [coord(coords, dims, 0), coord(coords, dims, 1), coord(coords, dims, CIRCLE_POINTS - 1)];
        if circular::center(three).is_none() {
            return Err(spatial(SqlState::SPATIAL_POINTS_ARE_COLLINEAR, "§7.4.3 Description 4d", "points are collinear"));
        }
        return Ok(());
    }
    for start in (0..count.saturating_sub(2)).step_by(2) {
        if same(start, start.saturating_add(2)) {
            return Err(duplicate());
        }
    }
    Ok(())
}

/// [§7.2.3, §7.3.3, §7.4.3, §7.5.3, §7.10.3, §8.4.3] `g` with its points
/// or curves replaced by the elements of `array`.
pub(super) fn replace(m: Mutator, g: &Geom, array: &Value) -> Result<Geom> {
    if m == Mutator::Curves {
        let (all, dims) = elements(m, g, array, false)?;
        let mut curves = Vec::new();
        for c in all {
            let Body::Curve(c) = c.body else { return Err(signature(m.name())) };
            curves.push(c);
        }
        let Body::Curve(Curve::Compound(_)) = &g.body else { return Err(signature(m.name())) };
        return Geom::new(g.srid, dims, Body::Curve(Curve::Compound(curves)));
    }
    let (points, dims) = elements(m, g, array, true)?;
    let coords = coordinates(m, &points)?;
    let body = match &g.body {
        Body::Curve(Curve::LineString(_)) => Body::Curve(Curve::LineString(coords)),
        Body::Curve(Curve::GeodesicString(_)) => Body::Curve(Curve::GeodesicString(coords)),
        Body::Curve(Curve::CircularString(_)) => {
            arcs(g, &coords, dims)?;
            Body::Curve(Curve::CircularString(coords))
        }
        Body::Curve(Curve::Circle(_)) => {
            arcs(g, &coords, dims)?;
            Body::Curve(Curve::Circle(coords))
        }
        Body::Surface(Surface::Triangle(_)) => {
            if points.len() != TRIANGLE_POINTS {
                return Err(invalid("§8.4.3 Description 4b", "a triangle's ring has four points"));
            }
            Body::Surface(Surface::Triangle(coords))
        }
        Body::Point(_) | Body::Curve(_) | Body::Surface(_) | Body::Solid(_) | Body::Collection(_) => {
            return Err(signature(m.name()));
        }
    };
    Geom::new(g.srid, dims, body)
}
