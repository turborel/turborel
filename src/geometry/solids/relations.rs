//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.9, §5.1.14 to §5.1.18,
//! §5.1.31 to §5.1.49] The relationships, set operations, simplicity,
//! boundary and measure locations of every geometry in space.

use super::units::in_unit;
use super::{number, spatial, text};
use crate::error::{Result, SqlState};
use crate::geometry::model::{Body, Collection, Coords, Curve, Geom, Surface};
use crate::spatial::curves::adapt::from_model;
use crate::spatial::curves::geodesy::Planar;
use crate::spatial::curves::observe;
use crate::spatial::curves::piece::Metric;
use crate::spatial::solids::ops::{SetOp, equals, intersects, set_op};
use crate::spatial::solids::shape::{boundary as boundary_of, patches_simple};
use crate::value::{Decimal, Value};

/// An INTEGER of a truth.
pub fn truth(b: bool) -> Value {
    Value::Exact(Decimal::new(i128::from(b), 0))
}

/// The other geometry of a pair, of the same reference system and, like
/// SELF, with z.
///
/// # Errors
///
/// SQLSTATE 2FF72 unless both have z; 2FF10 for different systems.
fn other(g: &Geom, rest: &[Value]) -> Result<Geom> {
    let h = Geom::from_value(rest.first().unwrap_or(&Value::Null))?;
    if !g.dims.has_z() || !h.dims.has_z() {
        return Err(spatial(SqlState::SPATIAL_BOTH_GEOMETRIES_MUST_BE_3D, "§5.1.31 Description 3b", "both geometries must be 3D"));
    }
    if g.srid != h.srid {
        return Err(spatial(
            SqlState::SPATIAL_MIXED_SPATIAL_REFERENCE_SYSTEMS,
            "§5.1",
            "the geometries are of different reference systems",
        ));
    }
    Ok(h)
}

/// §5.1.31, §5.1.34, §5.1.36, §5.1.38: the set operation `op` of SELF
/// `g` and the other geometry.
///
/// # Errors
///
/// As `other` and the 3D engine.
pub fn set(op: SetOp, g: &Geom, rest: &[Value]) -> Result<Value> {
    let h = other(g, rest)?;
    Ok(set_op(op, g, &h)?.to_value())
}

/// §5.1.42 ST_3DDistance: null if either is empty, else in SELF's linear
/// unit or the one named.
///
/// # Errors
///
/// As `other` and `in_unit`.
pub fn distance(g: &Geom, rest: &[Value]) -> Result<Value> {
    let h = other(g, rest)?;
    let unit = rest.get(1).map(text).transpose()?;
    let Some(d) = crate::spatial::solids::distance::distance(g, &h)? else { return Ok(Value::Null) };
    Ok(Value::Approx(in_unit(d, g.srid, unit, 1)?))
}

/// §5.1.44, §5.1.47, §5.1.49: whether the pair are equal, disjoint or
/// meet, as `which` says, 0 to 2.
///
/// # Errors
///
/// As `other` and the 3D engine.
pub fn relation(which: u8, g: &Geom, rest: &[Value]) -> Result<Value> {
    let h = other(g, rest)?;
    Ok(truth(match which {
        0 => equals(g, &h)?,
        1 => !intersects(g, &h)?,
        _ => intersects(g, &h)?,
    }))
}

/// §5.1.18 ST_3DBoundary: null of the empty set.
///
/// # Errors
///
/// As the 3D engine.
pub fn boundary(g: &Geom) -> Result<Value> {
    Ok(boundary_of(g)?.map_or(Value::Null, |b| b.to_value()))
}

/// §5.1.9 ST_3DIsSimple: 1 of the empty set; points that are not twice
/// the same; curves that do not cross themselves in space; multicurves
/// whose curves meet only where both end; polygons, as they are valid;
/// patch surfaces, multisurfaces and solids whose patches meet only along
/// their rims; collections whose elements are simple.
///
/// # Errors
///
/// As the 3D engine.
pub fn is_simple(g: &Geom) -> Result<Value> {
    Ok(truth(g.is_empty() || simple(&linear(g)?)?))
}

fn simple(g: &Geom) -> Result<bool> {
    let dims = g.dims;
    Ok(match &g.body {
        Body::Point(_) | Body::Surface(Surface::Polygon(_) | Surface::CurvePolygon(_)) => true,
        Body::Curve(c) => observe::is_simple(&from_model(c, dims), Metric::Space, &Planar)?,
        Body::Surface(s) => patches_simple(&[s], dims)?,
        Body::Solid(shells) => patches_simple(&shells.iter().collect::<Vec<_>>(), dims)?,
        Body::Collection(Collection::MultiPoint(points)) => distinct(points),
        Body::Collection(Collection::MultiCurve(curves)) => curves_simple(g, curves.iter().cloned().map(Body::Curve).collect())?,
        Body::Collection(Collection::MultiLineString(lines)) => {
            curves_simple(g, lines.iter().cloned().map(|l| Body::Curve(Curve::LineString(l))).collect())?
        }
        Body::Collection(Collection::MultiSurface(all)) => patches_simple(&all.iter().collect::<Vec<_>>(), dims)?,
        Body::Collection(Collection::MultiPolygon(all)) => {
            let surfaces: Vec<Surface> = all.iter().cloned().map(Surface::Polygon).collect();
            patches_simple(&surfaces.iter().collect::<Vec<_>>(), dims)?
        }
        Body::Collection(Collection::Geometry(parts)) => {
            let mut all = true;
            for part in parts {
                all = all && simple(&g.part(part.clone()))?;
            }
            all
        }
    })
}

/// Whether no two of `points` are the same in space.
fn distinct(points: &[Coords]) -> bool {
    let mut seen: Vec<&Coords> = points.iter().filter(|p| !p.is_empty()).collect();
    let count = seen.len();
    seen.sort_by(|p, q| p.iter().zip(q.iter()).map(|(a, b)| a.total_cmp(b)).find(|o| o.is_ne()).unwrap_or(std::cmp::Ordering::Equal));
    seen.dedup();
    seen.len() == count
}

/// Whether the curves `parts` of `g` are each simple and meet only at
/// points on both their boundaries.
fn curves_simple(g: &Geom, parts: Vec<Body>) -> Result<bool> {
    let curves: Vec<Geom> = parts.into_iter().map(|b| g.part(b)).collect();
    for (at, one) in curves.iter().enumerate() {
        if !simple(one)? {
            return Ok(false);
        }
        let ends = boundary_of(one)?;
        for two in curves.iter().skip(at.saturating_add(1)) {
            let meet = set_op(SetOp::Intersection, one, two)?;
            if meet.is_empty() {
                continue;
            }
            let theirs = boundary_of(two)?;
            let on = |b: &Option<Geom>| b.as_ref().map_or(Ok(false), |b| equals(&set_op(SetOp::Intersection, &meet, b)?, &meet));
            if !on(&ends)? || !on(&theirs)? {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// §5.1.14 ST_3DLocateAlong and §5.1.16 ST_3DLocateBetween: the parts of
/// SELF whose measures lie from the first measure to the second, z
/// included; an empty point if there are none, SELF has no measures, or
/// SELF is a surface or solid, for which it is implementation-defined.
///
/// # Errors
///
/// SQLSTATE 2FF02 if the second measure is below the first.
pub fn locate(g: &Geom, rest: &[Value]) -> Result<Value> {
    let start = number(rest.first().unwrap_or(&Value::Null))?;
    let end = rest.get(1).map_or(Ok(start), number)?;
    if end < start {
        return Err(spatial(SqlState::SPATIAL_INVALID_ARGUMENT, "§5.1.16 Description 2a", "the end measure is below the start measure"));
    }
    if g.is_empty() {
        return Ok(Value::Null);
    }
    let Some(m) = g.dims.m_at() else { return Ok(g.part(Body::Point(Vec::new())).to_value()) };
    let mut found = super::locate::Located::new(g.dims, m, start, end);
    found.walk(&linear(g)?.body);
    Ok(g.part(found.body()).to_value())
}

/// `g` with its curves line strings and its curve polygons polygons;
/// patch surfaces and solids as they are, which the 3D engine takes.
///
/// # Errors
///
/// As [`Geom::linear`].
pub fn linear(g: &Geom) -> Result<Geom> {
    match &g.body {
        Body::Solid(_)
        | Body::Surface(
            Surface::Polygon(_) | Surface::Triangle(_) | Surface::PolyhdrlSurface(_) | Surface::Tin(_) | Surface::Compound(_),
        ) => Ok(g.clone()),
        Body::Surface(Surface::CurvePolygon(_)) | Body::Point(_) | Body::Curve(_) | Body::Collection(_) => g.linear(),
    }
}
