//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.2.1.6] A value's linear
//! form, which planar algorithms take: points, linestrings and polygons
//! and their collections, of the value's dimensions. A triangle is its
//! polygon, a polyhedral surface, a TIN or a compound surface the
//! multipolygon of its patches, a compound curve the linestring of its
//! curves, a curve polygon the polygon of its rings, a multicurve and a
//! multisurface the multilinestring and multipolygon of their elements.
//! A curve of another interpolation is given to `CURVE_LINEARIZER`.

use super::check::limited;
use super::{Body, Collection, Coords, Curve, Dims, Geom, Rings, Surface};
use crate::error::{Code, Result, SqlError};
use crate::geometry::MAX_DEPTH;

/// \[DESIGN\] The linestring a curve of another interpolation than a
/// linestring's is approximated by, its first point its start and its last
/// its end: ST_CurveToLine of the curve engines at their default
/// tolerance (RULES.md D-SPATIAL-CURVES). None of a curve it does not take.
pub const CURVE_LINEARIZER: fn(&Curve, Dims) -> Option<Result<Coords>> = crate::spatial::curves::adapt::linearizer;

pub(super) fn of(g: &Geom) -> Result<Geom> {
    Ok(g.part(body(&g.body, g.dims, 1)?))
}

/// SQLCODE -900: a value no planar routine takes yet.
fn unlinear(kind: &str) -> SqlError {
    SqlError::new(
        Code::Unsupported,
        format!("[DESIGN] this spatial routine does not yet take a value of {kind} (RULES.md D-SPATIAL-MODEL)"),
    )
}

fn body(b: &Body, dims: Dims, depth: usize) -> Result<Body> {
    limited(depth, MAX_DEPTH)?;
    let deeper = depth.saturating_add(1);
    Ok(match b {
        Body::Point(_) | Body::Surface(Surface::Polygon(_)) => b.clone(),
        Body::Curve(c) => Body::Curve(Curve::LineString(curve(c, dims, deeper)?)),
        Body::Surface(s) => match polygons(s, dims, deeper)? {
            Polygons::One(rings) => Body::Surface(Surface::Polygon(rings)),
            Polygons::Many(all) => Body::Collection(Collection::MultiPolygon(all)),
        },
        Body::Solid(_) => return Err(unlinear("ST_BRepSolid")),
        Body::Collection(c) => Body::Collection(match c {
            Collection::Geometry(parts) => Collection::Geometry(parts.iter().map(|p| body(p, dims, deeper)).collect::<Result<_>>()?),
            Collection::MultiPoint(_) | Collection::MultiLineString(_) | Collection::MultiPolygon(_) => c.clone(),
            Collection::MultiCurve(curves) => {
                Collection::MultiLineString(curves.iter().map(|one| curve(one, dims, deeper)).collect::<Result<_>>()?)
            }
            Collection::MultiSurface(surfaces) => {
                let mut all = Vec::new();
                for s in surfaces {
                    match polygons(s, dims, deeper)? {
                        Polygons::One(rings) => all.push(rings),
                        Polygons::Many(more) => all.extend(more),
                    }
                }
                Collection::MultiPolygon(all)
            }
        }),
    })
}

/// A curve's points as a linestring's.
fn curve(c: &Curve, dims: Dims, depth: usize) -> Result<Coords> {
    limited(depth, MAX_DEPTH)?;
    match c {
        Curve::LineString(p) => Ok(p.clone()),
        Curve::Compound(parts) => {
            let stride = dims.stride();
            let mut out: Coords = Vec::new();
            for part in parts {
                let points = curve(part, dims, depth.saturating_add(1))?;
                // Each next curve starts where the last ended (§7.10).
                let skip = if out.is_empty() { 0 } else { stride };
                out.extend(points.iter().skip(skip));
            }
            Ok(out)
        }
        Curve::CircularString(_)
        | Curve::Circle(_)
        | Curve::GeodesicString(_)
        | Curve::Elliptical(_)
        | Curve::Nurbs(_)
        | Curve::Clothoid(_)
        | Curve::Spiral(_) => {
            if c.is_empty() {
                return Ok(Vec::new());
            }
            CURVE_LINEARIZER(c, dims).unwrap_or_else(|| Err(unlinear(c.kind().sql_name())))
        }
    }
}

/// A surface's polygons: one, or a collection's.
enum Polygons {
    One(Rings),
    Many(Vec<Rings>),
}

fn polygons(s: &Surface, dims: Dims, depth: usize) -> Result<Polygons> {
    limited(depth, MAX_DEPTH)?;
    Ok(match s {
        Surface::Polygon(rings) => Polygons::One(rings.clone()),
        Surface::CurvePolygon(rings) => {
            Polygons::One(rings.iter().map(|r| curve(r, dims, depth.saturating_add(1))).collect::<Result<_>>()?)
        }
        Surface::Triangle(ring) => Polygons::One(if ring.is_empty() { Vec::new() } else { vec![ring.clone()] }),
        Surface::PolyhdrlSurface(patches) => Polygons::Many(patches.clone()),
        Surface::Tin(tin) => Polygons::Many(tin.triangles.iter().map(|t| vec![t.clone()]).collect()),
        Surface::Compound(parts) => {
            let mut all = Vec::new();
            for part in parts {
                match polygons(part, dims, depth.saturating_add(1))? {
                    Polygons::One(rings) => all.push(rings),
                    Polygons::Many(more) => all.extend(more),
                }
            }
            Polygons::Many(all)
        }
    })
}
