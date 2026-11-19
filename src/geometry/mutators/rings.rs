//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.2.3, §8.2.4, §8.3.3,
//! §8.3.4, §8.4.5, §8.4.6, §8.5.3, §8.6.7] The mutators of a surface's
//! rings and patches: ST_ExteriorRing(ST_Curve) and
//! ST_InteriorRings(ST_Curve ARRAY) of a curve polygon, a polygon and a
//! triangle, and ST_Patches of a polyhedral surface and a TIN. A ring is
//! a closed curve, and the rings together make a valid polygon, read in x
//! and y (`spatial::planar::is_valid`). The patches meet as §8.5.3
//! Description 4e and 4f ask, read by their edges: every patch has an
//! edge another patch has, and no edge is of three patches.

use std::collections::HashMap;

use super::{invalid, mixed_systems};
use crate::error::{Result, SqlState};
use crate::geometry::engine::engine_of;
use crate::geometry::model::{Body, Coords, Curve, Dims, Geom, Rings, Surface, Tin};
use crate::geometry::observers::signature;
use crate::geometry::representations::spatial;
use crate::geometry::support::{checked_geometries, same_dims};
use crate::value::Value;

const EXTERIOR: &str = "ST_EXTERIORRING";
const INTERIOR: &str = "ST_INTERIORRINGS";
const PATCHES: &str = "ST_PATCHES";
/// The points of a triangle's ring [§8.4.5 Description 4b].
const TRIANGLE_POINTS: usize = 4;

/// 2FF25 mixed coordinate dimensions, citing `clause`.
fn mixed_dims(clause: &str) -> crate::error::SqlError {
    spatial(SqlState::SPATIAL_MIXED_COORDINATE_DIMENSIONS, clause, "mixed coordinate dimensions")
}

/// The rings of a curve polygon, a polygon or a triangle, exterior first.
fn rings_of(g: &Geom) -> Result<Vec<Curve>> {
    match &g.body {
        Body::Surface(Surface::CurvePolygon(rings)) => Ok(rings.clone()),
        Body::Surface(Surface::Polygon(rings)) => Ok(rings.iter().map(|r| Curve::LineString(r.clone())).collect()),
        Body::Surface(Surface::Triangle(ring)) => Ok(if ring.is_empty() { Vec::new() } else { vec![Curve::LineString(ring.clone())] }),
        Body::Point(_) | Body::Curve(_) | Body::Surface(_) | Body::Solid(_) | Body::Collection(_) => Err(signature(EXTERIOR)),
    }
}

/// [§8.2.3 Description 4d] Whether `c`, of `dims`, is a ring: closed.
fn closed(c: &Curve, dims: Dims, srid: i32) -> Result<bool> {
    let line = Geom { srid, dims, body: Body::Curve(c.clone()) }.linear()?;
    let Body::Curve(Curve::LineString(points)) = &line.body else { return Ok(false) };
    let stride = dims.planar();
    let (first, last) =
        (points.get(..stride), points.len().checked_sub(dims.stride()).and_then(|at| points.get(at..at.saturating_add(stride))));
    Ok(!points.is_empty() && first == last)
}

/// The surface of `g`'s type of the rings `rings`, of `dims`, each a
/// ring, together a valid polygon.
fn surface(g: &Geom, dims: Dims, rings: Vec<Curve>, clause: &str) -> Result<Geom> {
    for r in &rings {
        if !closed(r, dims, g.srid)? {
            return Err(invalid(clause, "a ring is a closed curve"));
        }
    }
    let lines = || -> Result<Rings> {
        rings.iter().map(|r| if let Curve::LineString(c) = r { Ok(c.clone()) } else { Err(signature(EXTERIOR)) }).collect()
    };
    let body = match &g.body {
        Body::Surface(Surface::Polygon(_)) => Surface::Polygon(lines()?),
        Body::Surface(Surface::Triangle(_)) => Surface::Triangle(lines()?.into_iter().next().unwrap_or_default()),
        Body::Point(_) | Body::Curve(_) | Body::Surface(_) | Body::Solid(_) | Body::Collection(_) => Surface::CurvePolygon(rings),
    };
    let made = Geom::new(g.srid, dims, Body::Surface(body))?;
    if !crate::spatial::planar::is_valid(&engine_of(&made)?)? {
        return Err(invalid(clause, "the rings make a valid polygon"));
    }
    Ok(made)
}

/// [§8.3.3 Description 4a, §8.4.5 Description 4a, 4b] A polygon's or a
/// triangle's ring is a linestring, a triangle's of four points.
fn linestring(g: &Geom, c: Curve, clause: &str) -> Result<Curve> {
    let polygon = matches!(&g.body, Body::Surface(Surface::Polygon(_) | Surface::Triangle(_)));
    let points = if let Curve::LineString(points) = &c { Some(points.len()) } else { None };
    match (polygon, points) {
        (false, _) => Ok(c),
        (true, None) => Err(spatial(SqlState::SPATIAL_CURVE_NOT_A_LINESTRING, clause, "curve value is not a linestring value")),
        (true, Some(n)) => {
            let triangle = matches!(&g.body, Body::Surface(Surface::Triangle(_)));
            if triangle && n != TRIANGLE_POINTS.saturating_mul(g.dims.stride()) {
                return Err(spatial(
                    SqlState::SPATIAL_EXTERIOR_RING_NOT_FOUR_POINTS,
                    "§8.4.5 Description 4b",
                    "exterior ring must have exactly 4 points",
                ));
            }
            Ok(c)
        }
    }
}

/// [§8.2.3, §8.3.3, §8.4.5] ST_ExteriorRing(ST_Curve).
pub(super) fn exterior(g: &Geom, arg: &Value) -> Result<Geom> {
    const CLAUSE: &str = "§8.2.3 Description 4";
    let ring = Geom::from_value(arg)?;
    if ring.srid != g.srid {
        return Err(mixed_systems(CLAUSE));
    }
    let dims = ring.dims;
    let interiors: Vec<Curve> = rings_of(g)?.into_iter().skip(1).collect();
    if !interiors.is_empty() && dims != g.dims {
        return Err(mixed_dims(CLAUSE));
    }
    let Body::Curve(c) = ring.body else { return Err(signature(EXTERIOR)) };
    let c = linestring(&Geom { dims, ..g.clone() }, c, "§8.3.3 Description 4a")?;
    surface(g, dims, std::iter::once(c).chain(interiors).collect(), CLAUSE)
}

/// [§8.2.4, §8.3.4, §8.4.6] ST_InteriorRings(ST_Curve ARRAY).
pub(super) fn interior(g: &Geom, array: &Value) -> Result<Geom> {
    const CLAUSE: &str = "§8.2.4 Description 4";
    let rings = rings_of(g)?;
    let Some(exterior) = rings.first().cloned() else {
        return Err(spatial(SqlState::SPATIAL_NULL_EXTERIOR_RING, "§8.2.4 Description 4a", "null exterior ring"));
    };
    let (all, srid) = checked_geometries(array, false)?;
    if let (Body::Surface(Surface::Triangle(_)), false) = (&g.body, all.is_empty()) {
        return Err(spatial(SqlState::SPATIAL_TRIANGLES_CANNOT_HAVE_HOLES, "§8.4.6 Description 4a", "triangles cannot have holes"));
    }
    if !all.is_empty() && srid != g.srid {
        return Err(mixed_systems(CLAUSE));
    }
    same_dims(&all)?;
    if all.first().is_some_and(|r| r.dims != g.dims) {
        return Err(mixed_dims(CLAUSE));
    }
    let polygon = matches!(&g.body, Body::Surface(Surface::Polygon(_)));
    let mut out = vec![exterior];
    for ring in all {
        let Body::Curve(c) = ring.body else { return Err(signature(INTERIOR)) };
        if polygon && !matches!(c, Curve::LineString(_)) {
            return Err(spatial(
                SqlState::SPATIAL_ELEMENT_IS_NOT_A_VALID_TYPE,
                "§8.3.4 Description 4a",
                "element is not an ST_LineString type",
            ));
        }
        out.push(c);
    }
    surface(g, g.dims, out, CLAUSE)
}

/// The rings of a patch: a polygon's, or a triangle's one.
fn patch_rings(p: Geom, tin: bool) -> Result<Rings> {
    match (p.body, tin) {
        (Body::Surface(Surface::Triangle(ring)), _) => Ok(vec![ring]),
        (Body::Surface(Surface::Polygon(rings)), false) => Ok(rings),
        (Body::Point(_) | Body::Curve(_) | Body::Surface(_) | Body::Solid(_) | Body::Collection(_), true) => {
            Err(spatial(SqlState::SPATIAL_POLYGON_NOT_A_TRIANGLE, "§8.6.7 Description 4a", "polygon value is not a triangle value"))
        }
        (Body::Point(_) | Body::Curve(_) | Body::Surface(_) | Body::Solid(_) | Body::Collection(_), false) => Err(signature(PATCHES)),
    }
}

/// The edges of a ring of `stride`, each its two points' bits in order.
fn edges(ring: &Coords, stride: usize) -> Vec<(Vec<u64>, Vec<u64>)> {
    let points: Vec<Vec<u64>> = ring.chunks_exact(stride).map(|p| p.iter().map(|x| (x + 0.0_f64).to_bits()).collect()).collect();
    points
        .windows(2)
        .filter_map(|w| if let [a, b] = w { Some(if a <= b { (a.clone(), b.clone()) } else { (b.clone(), a.clone()) }) } else { None })
        .collect()
}

/// [§8.5.3 Description 4e, 4f] Whether the patches meet: each has an
/// edge of another, if there are two, and no edge is of three.
fn meet(patches: &[Rings], dims: Dims) -> bool {
    let mut of: HashMap<(Vec<u64>, Vec<u64>), Vec<usize>> = HashMap::new();
    for (i, rings) in patches.iter().enumerate() {
        for e in rings.iter().flat_map(|r| edges(r, dims.stride())) {
            let at = of.entry(e).or_default();
            if !at.contains(&i) {
                at.push(i);
            }
        }
    }
    let shared = |i: usize| of.values().any(|at| at.len() > 1 && at.contains(&i));
    of.values().all(|at| at.len() <= 2) && (patches.len() < 2 || (0..patches.len()).all(shared))
}

/// [§8.5.3, §8.6.7] ST_Patches(ST_Polygon ARRAY), ST_Patches(ST_Triangle
/// ARRAY) of a TIN.
pub(super) fn patches(g: &Geom, array: &Value) -> Result<Geom> {
    const CLAUSE: &str = "§8.5.3 Description 4";
    let tin = matches!(&g.body, Body::Surface(Surface::Tin(_)));
    let (all, srid) = checked_geometries(array, false)?;
    let dims = same_dims(&all).map(|_| all.first().map_or(Dims::Xyz, |p| p.dims))?;
    if !dims.has_z() {
        return Err(invalid("§8.5.3 Description 4b", "the patches are 3D"));
    }
    if !all.is_empty() && srid != g.srid {
        return Err(mixed_systems(CLAUSE));
    }
    let patches: Vec<Rings> = all.into_iter().map(|p| patch_rings(p, tin)).collect::<Result<_>>()?;
    if !meet(&patches, dims) {
        return Err(invalid("§8.5.3 Description 4e, 4f", "each patch meets another in a line, and no line is of three"));
    }
    let body = match &g.body {
        Body::Surface(Surface::Tin(old)) => {
            let triangles = patches.into_iter().map(|mut rings| rings.pop().unwrap_or_default()).collect();
            Surface::Tin(Tin { triangles, ..old.clone() })
        }
        Body::Point(_) | Body::Curve(_) | Body::Surface(_) | Body::Solid(_) | Body::Collection(_) => Surface::PolyhdrlSurface(patches),
    };
    Geom::new(g.srid, dims, Body::Surface(body))
}
