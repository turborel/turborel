//! \[DESIGN\] The TIN routines of ISO/IEC 13249-3:2016 (DIS, SC32 N2593)
//! §8.6.4 and §8.6.6. A new maximum side length retriangulates through
//! the curve engine's constrained triangulation, from the elements, or
//! from the triangles' corners when there are none. A clip cuts each
//! triangle's plan by the polygon with the planar engine, triangulates
//! what is left and lifts it back onto the triangle's plane; the polygon
//! then replaces the boundary elements, draped on the surface.

use super::cells::P3;
use super::fault;
use super::triangulate::polygon;
use crate::error::{Result, SqlState};
use crate::geometry::model::{Dims, Geom};
use crate::spatial::curves::tin::{Constraint, ElementKind, triangulate};
use crate::spatial::curves::types::Coord;
use crate::spatial::formats::model::{from_geom, to_geom};
use crate::spatial::formats::tree::{Body, Element, Geometry, Surface, Tin, constraint};
use crate::spatial::planar;

/// The tree of the TIN `g`.
///
/// # Errors
///
/// SQLSTATE 2FF71 if `g` is no TIN.
pub fn tin_of(g: &Geom) -> Result<Geometry> {
    let tree = from_geom(g)?;
    if matches!(tree.body, Body::Surface(Surface::Tin(_))) {
        return Ok(tree);
    }
    Err(fault(SqlState::SPATIAL_NOT_A_TIN, "§8.6", "the value is not a TIN"))
}

/// The TIN of a tree `tin_of` gave.
pub fn tin_in(tree: &mut Geometry) -> Option<&mut Tin> {
    if let Body::Surface(Surface::Tin(t)) = &mut tree.body { Some(t) } else { None }
}

/// §8.6.4 ST_MaxSideLength(DOUBLE PRECISION, INTEGER): `g` holding
/// triangles' sides to `length`, retriangulated if `again`.
///
/// # Errors
///
/// SQLSTATE 2FF02 if `length` is not above zero; the triangulation's.
pub fn with_max_side(g: &Geom, length: f64, again: bool) -> Result<Geom> {
    if !length.is_finite() || length <= 0.0_f64 {
        return Err(fault(SqlState::SPATIAL_INVALID_ARGUMENT, "§8.6.4 Description 4b", "the maximum side length is not above 0"));
    }
    let mut tree = tin_of(g)?;
    let dims = tree.dims;
    if let Some(tin) = tin_in(&mut tree) {
        let all = constraints(tin, dims);
        if again && all.iter().any(|c| !c.parts.is_empty()) {
            tin.triangles = triangulate(&all, Some(length))?.into_iter().map(|[a, b, c]| vec![a, b, c, a]).collect();
        }
        tin.max_side = Some(length);
    }
    to_geom(&tree, g.srid)
}

/// What the TIN is triangulated from: its elements, or its triangles'
/// corners as random points when it has none.
fn constraints(tin: &Tin, dims: Dims) -> Vec<Constraint> {
    if tin.elements.is_empty() {
        let parts = tin.triangles.iter().flat_map(|r| r.iter().take(3)).map(|p| vec![*p]).collect();
        return vec![Constraint { kind: ElementKind::RandomPoints, parts, has_z: dims.has_z() }];
    }
    tin.elements.iter().map(|e| constraint(e, dims)).collect()
}

/// §8.6.6 ST_Clip(ST_Polygon): what of the TIN `g` lies within the
/// polygon `window`, which becomes its boundary.
///
/// # Errors
///
/// SQLSTATE 2FF71 if `g` is no TIN; the planar engine's and the
/// triangulation's.
pub fn clip(g: &Geom, window: &[Vec<[f64; 2]>]) -> Result<Geom> {
    let mut tree = tin_of(g)?;
    let shape = planar::Geometry::Polygon(window.to_vec());
    let Some(tin) = tin_in(&mut tree) else { return to_geom(&tree, g.srid) };
    let corners: Vec<[Coord; 3]> = tin.triangles.iter().filter_map(|r| corners_of(r)).collect();
    let mut kept = Vec::new();
    for t in &corners {
        let plan = planar::Geometry::Polygon(vec![t.iter().chain(t.first()).map(|p| [p.x, p.y]).collect()]);
        for piece in polygons(planar::intersection(&plan, &shape)?) {
            let flat: Vec<Vec<P3>> = piece.iter().map(|r| r.iter().map(|p| [p[0], p[1], 0.0_f64]).collect()).collect();
            kept.extend(polygon(&flat)?.into_iter().filter_map(|[a, b, c]| counterclockwise([a, b, c].map(|p| lift(*t, p)))));
        }
    }
    tin.triangles = kept.into_iter().map(|[a, b, c]| vec![a, b, c, a]).collect();
    let rings = window.iter().map(|r| r.iter().map(|p| drape(&corners, *p)).collect()).collect();
    tin.elements.retain(|e| e.kind != ElementKind::Boundary);
    tin.elements.push(Element { kind: ElementKind::Boundary, id: 0, tag: String::new(), body: Body::Surface(Surface::Polygon(rings)) });
    to_geom(&tree, g.srid)
}

/// The first three points of a triangle ring.
fn corners_of(ring: &[Coord]) -> Option<[Coord; 3]> {
    match ring {
        [a, b, c, ..] => Some([*a, *b, *c]),
        _ => None,
    }
}

/// The polygons of a planar result, its rings each.
fn polygons(g: planar::Geometry) -> Vec<Vec<Vec<[f64; 2]>>> {
    match g {
        planar::Geometry::Polygon(rings) => vec![rings],
        planar::Geometry::MultiPolygon(all) => all,
        planar::Geometry::Collection(parts) => parts.into_iter().flat_map(polygons).collect(),
        planar::Geometry::Empty
        | planar::Geometry::Point(_)
        | planar::Geometry::Line(_)
        | planar::Geometry::MultiPoint(_)
        | planar::Geometry::MultiLine(_) => Vec::new(),
    }
}

/// Twice the signed plan area of `a`, `b`, `c`.
fn plan_area(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

/// The triangle turned counterclockwise in plan; none if it has no
/// plan area.
fn counterclockwise([a, b, c]: [Coord; 3]) -> Option<[Coord; 3]> {
    let area = plan_area([a.x, a.y], [b.x, b.y], [c.x, c.y]);
    if area > 0.0_f64 {
        return Some([a, b, c]);
    }
    (area < 0.0_f64).then_some([a, c, b])
}

/// The point of the triangle `t`'s plane over `p`'s plan position, z and
/// m interpolated; a corner itself when `p` is on it.
fn lift(corners: [Coord; 3], p: P3) -> Coord {
    let [a, b, c] = corners;
    if let Some(corner) = corners.iter().find(|q| q.x.total_cmp(&p[0]).is_eq() && q.y.total_cmp(&p[1]).is_eq()) {
        return *corner;
    }
    let whole = plan_area([a.x, a.y], [b.x, b.y], [c.x, c.y]);
    if whole == 0.0_f64 {
        return Coord { x: p[0], y: p[1], ..a };
    }
    let wb = plan_area([a.x, a.y], [p[0], p[1]], [c.x, c.y]) / whole;
    let wc = plan_area([a.x, a.y], [b.x, b.y], [p[0], p[1]]) / whole;
    let wa = 1.0_f64 - wb - wc;
    Coord { x: p[0], y: p[1], z: wa * a.z + wb * b.z + wc * c.z, m: wa * a.m + wb * b.m + wc * c.m }
}

/// `p` draped on the triangles `all`: lifted by the one whose plan holds
/// it, else given the z and m of the nearest corner in plan.
fn drape(all: &[[Coord; 3]], p: [f64; 2]) -> Coord {
    let holds = |t: &&[Coord; 3]| {
        let [a, b, c] = t.map(|q| [q.x, q.y]);
        let sides = [planar::orient2d(a, b, p), planar::orient2d(b, c, p), planar::orient2d(c, a, p)];
        !sides.contains(&std::cmp::Ordering::Less) || !sides.contains(&std::cmp::Ordering::Greater)
    };
    if let Some(t) = all.iter().find(holds) {
        return lift(*t, [p[0], p[1], 0.0_f64]);
    }
    let gap = |q: &Coord| (q.x - p[0]).hypot(q.y - p[1]);
    let nearest = all.iter().flatten().min_by(|q, r| gap(q).total_cmp(&gap(r)));
    nearest.map_or(Coord { x: p[0], y: p[1], z: 0.0_f64, m: 0.0_f64 }, |q| Coord { x: p[0], y: p[1], ..*q })
}
