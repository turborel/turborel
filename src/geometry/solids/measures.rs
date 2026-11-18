//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.1, §8.1, §8.4.4, §9.1,
//! §10.3, §10.5] The measures in space of curves, surfaces and solids. A
//! curve's come from the curve engine in space; a surface's from its
//! patches, each a planar polygon in space; a solid's from the 3D engine.
//! A measure without a unit is in the linear unit of SELF's reference
//! system; one with a unit is converted to it.

use super::relations::{linear, truth};
use super::units::in_unit;
use super::{number, spatial, text};
use crate::error::{Result, SqlState};
use crate::geometry::model::{Body, Collection, Curve, Dims, Geom, Surface};
use crate::spatial::curves::adapt::{coord, flat, from_model};
use crate::spatial::curves::geodesy::Planar;
use crate::spatial::curves::observe;
use crate::spatial::curves::patches::{self, Patch};
use crate::spatial::curves::piece::Metric;
use crate::spatial::curves::types::Coord;
use crate::spatial::solids::cells::patches as rings_in_space;
use crate::spatial::solids::measure;
use crate::spatial::solids::shape::{patches_simple, surface_is_closed};
use crate::value::Value;

/// The unit a call names after its other arguments, at `at`.
fn unit(rest: &[Value], at: usize) -> Result<Option<&str>> {
    rest.get(at).map(text).transpose()
}

/// The point of `c` in space, as a value like `g`, with z if `g` has it.
fn point(g: &Geom, c: Coord) -> Value {
    let dims = if g.dims.has_z() { Dims::Xyz } else { Dims::Xy };
    Geom { srid: g.srid, dims, body: Body::Point(flat(&[c], dims)) }.to_value()
}

/// The curves of a curve or multicurve.
fn curves(g: &Geom) -> Vec<Curve> {
    match &g.body {
        Body::Curve(c) => vec![c.clone()],
        Body::Collection(Collection::MultiCurve(all)) => all.clone(),
        Body::Collection(Collection::MultiLineString(all)) => all.iter().cloned().map(Curve::LineString).collect(),
        Body::Point(_) | Body::Surface(_) | Body::Solid(_) | Body::Collection(_) => Vec::new(),
    }
}

/// §7.1.3, §10.3.6 ST_3DLength: null of the empty set.
///
/// # Errors
///
/// As the curve engine and `in_unit`.
pub fn length(g: &Geom, rest: &[Value]) -> Result<Value> {
    if g.is_empty() {
        return Ok(Value::Null);
    }
    let mut total = 0.0_f64;
    for c in curves(g) {
        total += observe::length(&from_model(&c, g.dims), Metric::Space, &Planar)?.unwrap_or_default();
    }
    Ok(Value::Approx(in_unit(total, g.srid, unit(rest, 0)?, 1)?))
}

/// §7.1.7, §8.1.11, §10.3.4 ST_3DIsClosed: 0 of the empty set; a curve
/// whose ends meet in space, a multicurve of such curves, a surface with
/// no boundary.
///
/// # Errors
///
/// As the curve engine.
pub fn is_closed(g: &Geom) -> Result<Value> {
    if g.is_empty() {
        return Ok(truth(false));
    }
    if let Body::Surface(_) = g.body {
        return Ok(truth(surface_is_closed(&linear(g)?)));
    }
    let mut all = true;
    for c in curves(g) {
        all = all && observe::is_closed(&from_model(&c, g.dims), Metric::Space, &Planar)?;
    }
    Ok(truth(all))
}

/// §7.1.9 ST_3DIsRing: 0 of the empty set; a closed simple curve in
/// space.
///
/// # Errors
///
/// As the curve engine.
pub fn is_ring(g: &Geom) -> Result<Value> {
    let Body::Curve(c) = &g.body else { return Ok(truth(false)) };
    Ok(truth(!g.is_empty() && observe::is_ring(&from_model(c, g.dims), Metric::Space, &Planar)?))
}

/// §7.1.12 ST_3DDistanceToPt: the length along the curve from its start
/// to the point, in space.
///
/// # Errors
///
/// SQLSTATE 2FF10 for a point of another reference system; the curve
/// engine's, as 2FF80 when the curve misses the point.
pub fn distance_to_pt(g: &Geom, rest: &[Value]) -> Result<Value> {
    let p = Geom::from_value(rest.first().unwrap_or(&Value::Null))?;
    if p.srid != g.srid {
        return Err(spatial(SqlState::SPATIAL_MIXED_SPATIAL_REFERENCE_SYSTEMS, "§7.1.12", "the point is of another reference system"));
    }
    let (Body::Curve(c), Body::Point(at)) = (&g.body, &p.body) else { return Ok(Value::Null) };
    if at.is_empty() {
        return Ok(Value::Null);
    }
    let found = observe::distance_to_point(&from_model(c, g.dims), coord(at, p.dims), Metric::Space, &Planar)?;
    found.map_or(Ok(Value::Null), |d| Ok(Value::Approx(in_unit(d, g.srid, unit(rest, 1)?, 1)?)))
}

/// §7.1.16 ST_3DPtAtDistance: the point a length along the curve from
/// its start, in space; the length in the unit named, if one is.
///
/// # Errors
///
/// As the curve engine, 2FF79 past the curve's end; as `in_unit`.
pub fn pt_at_distance(g: &Geom, rest: &[Value]) -> Result<Value> {
    let Body::Curve(c) = &g.body else { return Ok(Value::Null) };
    let given = number(rest.first().unwrap_or(&Value::Null))?;
    let per_unit = in_unit(1.0_f64, g.srid, unit(rest, 1)?, 1)?;
    let found = observe::point_at_distance(&from_model(c, g.dims), given / per_unit, Metric::Space, &Planar)?;
    Ok(found.map_or(Value::Null, |p| point(g, p)))
}

/// The patches in space of each surface of a surface or multisurface.
fn surfaces(g: &Geom) -> Result<Vec<Vec<Patch>>> {
    let g = linear(g)?;
    let all: Vec<Surface> = match g.body {
        Body::Surface(s) => vec![s],
        Body::Collection(Collection::MultiSurface(all)) => all,
        Body::Collection(Collection::MultiPolygon(all)) => all.into_iter().map(Surface::Polygon).collect(),
        Body::Point(_) | Body::Curve(_) | Body::Solid(_) | Body::Collection(_) => Vec::new(),
    };
    let of = |s: &Surface| -> Vec<Patch> {
        rings_in_space(s, g.dims)
            .into_iter()
            .map(|p| p.into_iter().map(|r| r.into_iter().map(|[x, y, z]| Coord::xyz(x, y, z)).collect()).collect())
            .collect()
    };
    Ok(all.iter().map(of).collect())
}

/// §8.1.3, §10.5.4 ST_3DArea and §8.1.5, §10.5.6 ST_3DPerimeter, as
/// `perimeter` says: summed over the surfaces; null of the empty set.
///
/// # Errors
///
/// As `in_unit`.
pub fn area_or_perimeter(g: &Geom, rest: &[Value], perimeter: bool) -> Result<Value> {
    if g.is_empty() {
        return Ok(Value::Null);
    }
    let measure = |p: &[Patch]| if perimeter { patches::perimeter(p, Metric::Space) } else { patches::area(p, Metric::Space) };
    let total: f64 = surfaces(g)?.iter().filter_map(|s| measure(s)).sum();
    Ok(Value::Approx(in_unit(total, g.srid, unit(rest, 0)?, if perimeter { 1 } else { 2 })?))
}

/// §8.1.7, §9.1.4, §10.5.8 ST_3DCentroid: of a surface the centroid of
/// its area, of a solid of its volume; null of the empty set.
///
/// # Errors
///
/// As the 3D engine.
pub fn centroid(g: &Geom) -> Result<Value> {
    if g.is_empty() {
        return Ok(Value::Null);
    }
    if let Body::Solid(_) = g.body {
        return Ok(measure::centroid(&linear(g)?)?.map_or(Value::Null, |[x, y, z]| point(g, Coord::xyz(x, y, z))));
    }
    let all: Vec<Patch> = surfaces(g)?.into_iter().flatten().collect();
    Ok(patches::centroid(&all, Metric::Space).map_or(Value::Null, |c| point(g, c)))
}

/// §8.1.9, §10.5.10 ST_3DPointOnSurf: a point on the surface in space;
/// null of the empty set.
///
/// # Errors
///
/// As the curve engine's patches.
pub fn point_on_surf(g: &Geom) -> Result<Value> {
    if g.is_empty() {
        return Ok(Value::Null);
    }
    let all: Vec<Patch> = surfaces(g)?.into_iter().flatten().collect();
    Ok(patches::point_on_surface(&all, Metric::Space)?.map_or(Value::Null, |c| point(g, c)))
}

/// §8.1.13 ST_IsShell: 1 of a simple, closed surface; 0 of the empty set.
///
/// # Errors
///
/// As the 3D engine.
pub fn is_shell(g: &Geom) -> Result<Value> {
    let g = linear(g)?;
    let Body::Surface(s) = &g.body else { return Ok(truth(false)) };
    Ok(truth(!g.is_empty() && surface_is_closed(&g) && patches_simple(&[s], g.dims)?))
}

/// §8.4.4 ST_3DSlope: the triangle's rise over run; null of the empty
/// set.
///
/// # Errors
///
/// SQLSTATE 2FF02 for a vertical triangle, which has no slope.
pub fn slope(g: &Geom) -> Result<Value> {
    let Body::Surface(Surface::Triangle(ring)) = &g.body else { return Ok(Value::Null) };
    let corners: Vec<[f64; 3]> = ring.chunks(g.dims.stride()).take(3).map(|p| coord(p, g.dims).space()).collect();
    let [a, b, c] = corners.as_slice() else { return Ok(Value::Null) };
    measure::slope([*a, *b, *c])
        .map(Value::Approx)
        .ok_or_else(|| spatial(SqlState::SPATIAL_INVALID_ARGUMENT, "§8.4.4 Description 2", "a vertical triangle has no slope"))
}

/// §9.1.2 ST_3DSurfaceArea and §9.1.3 ST_3DVolume, as `volume` says;
/// null of the empty set.
///
/// # Errors
///
/// As the 3D engine and `in_unit`.
pub fn solid_measure(g: &Geom, rest: &[Value], volume: bool) -> Result<Value> {
    if g.is_empty() {
        return Ok(Value::Null);
    }
    let shells = linear(g)?;
    let (value, power) = if volume { (measure::volume(&shells)?, 3_i32) } else { (measure::surface_area(&shells)?, 2_i32) };
    Ok(Value::Approx(in_unit(value, g.srid, unit(rest, 0)?, power)?))
}

/// §9.1.5 ST_3DPointOnSolid: a point in the solid; null of the empty set.
///
/// # Errors
///
/// As the 3D engine.
pub fn point_on_solid(g: &Geom) -> Result<Value> {
    if g.is_empty() {
        return Ok(Value::Null);
    }
    Ok(measure::point_on(&linear(g)?)?.map_or(Value::Null, |[x, y, z]| point(g, Coord::xyz(x, y, z))))
}
