//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3] The geometry the
//! network routines read and make: a curve's end points, equality of
//! points as ST_Equals finds it, a point within a link, a curve cut at a
//! point, and two curves joined end to end.
//!
//! Reading: a link is cut and joined on its linear form (`Geom::linear`),
//! as a topology's edges are, so the parts of a link that is not a
//! linestring are linestrings through its linearized points.

use std::cmp::Ordering;

use super::damaged;
use crate::error::Result;
use crate::geometry::model::{Body, Coords, Curve, Dims, Geom};
use crate::spatial::curves::adapt::{flat, from_model};
use crate::spatial::curves::geodesy::Planar;
use crate::spatial::curves::path::path;
use crate::spatial::planar::{Point, orient2d};

/// [§7.1.4, §7.1.5] The start and end points of curve `g`, as points of
/// its system and dimensions; none of an empty curve.
///
/// # Errors
///
/// SQLCODE -916 if `g` is no curve; a curve's that is not well formed.
pub(super) fn ends(g: &Geom) -> Result<Option<(Geom, Geom)>> {
    let Body::Curve(c) = &g.body else { return Err(damaged("a link's geometry is no curve")) };
    let Some(p) = path(&from_model(c, g.dims), &Planar)? else { return Ok(None) };
    let point = |q| Geom::new(g.srid, g.dims, Body::Point(flat(&[q], g.dims)));
    Ok(Some((point(p.start)?, point(p.end)?)))
}

/// [§5.1.43] Whether point `a` equals point `b`, as ST_Equals finds.
///
/// # Errors
///
/// As ST_Equals's.
pub(super) fn equal(a: &Geom, b: &Geom) -> Result<bool> {
    crate::geometry::EQUALITY(a, b)
}

/// The points, with all their ordinates, of the linear form of curve `g`.
///
/// # Errors
///
/// SQLCODE -916 if `g` is no curve; the linearizer's.
pub(super) fn coords_of(g: &Geom) -> Result<Coords> {
    match g.linear()?.body {
        Body::Curve(Curve::LineString(c)) => Ok(c),
        Body::Point(_) | Body::Curve(_) | Body::Surface(_) | Body::Solid(_) | Body::Collection(_) => {
            Err(damaged("a link's geometry is no curve"))
        }
    }
}

/// The x and y of a point's ordinates `q`.
fn xy(q: &[f64]) -> Point {
    [q.first().copied().unwrap_or_default(), q.get(1).copied().unwrap_or_default()]
}

/// The x and y of point `g`, none if it is empty or no point.
pub(super) fn point_xy(g: &Geom) -> Option<Point> {
    let Body::Point(c) = &g.body else { return None };
    Some([*c.first()?, *c.get(1)?])
}

/// Whether `a` and `b` are one position. Ordinates are finite and never
/// negative zero (`Geom::new`), so equal bits are equal numbers.
fn same(a: Point, b: Point) -> bool {
    a.map(f64::to_bits) == b.map(f64::to_bits)
}

/// Whether `v` lies between `a` and `b`, either way round.
fn between(v: f64, a: f64, b: f64) -> bool {
    a.min(b) <= v && v <= a.max(b)
}

/// Whether `p` is on the segment from `a` to `b`, exactly.
fn on_segment(p: Point, a: Point, b: Point) -> bool {
    let ([px, py], [ax, ay], [bx, by]) = (p, a, b);
    orient2d(a, b, p) == Ordering::Equal && between(px, ax, bx) && between(py, ay, by)
}

/// [§5.1.53] Where point `p` lies within the linear curve of points
/// `coords`: the index of the first segment it is on. None if it is on
/// none or is one of the curve's ends. Reading: the ends of a curve that
/// is not closed are its boundary [§7.1.1], so not within it; those of a
/// closed one are within it, but a split there would make a part of no
/// length, which is no curve, so the point is not on the link for the
/// split either way.
pub(super) fn within(p: Point, coords: &Coords, dims: Dims) -> Option<usize> {
    let points: Vec<Point> = coords.chunks_exact(dims.stride()).map(xy).collect();
    if same(*points.first()?, p) || same(*points.last()?, p) {
        return None;
    }
    points.windows(2).position(|w| if let [a, b] = w { on_segment(p, *a, *b) } else { false })
}

/// The ordinates, all of `dims`, of the point at `p` within the segment
/// from `a` to `b`: those of `given` if it has the dimensions, else x and
/// y of `p` and the rest interpolated by the distance along the segment.
fn vertex(p: Point, (before, after): (&[f64], &[f64]), given: &Geom, dims: Dims) -> Vec<f64> {
    if given.dims == dims
        && let Body::Point(c) = &given.body
    {
        return c.clone();
    }
    let [px, py] = p;
    let along = |q: &[f64]| {
        let [qx, qy] = xy(q);
        (qx - px).hypot(qy - py)
    };
    let (da, db) = (along(before), along(after));
    let t = if da + db > 0.0_f64 { da / (da + db) } else { 0.0_f64 };
    let mut out = p.to_vec();
    out.extend(before.iter().zip(after).skip(2).map(|(u, v)| u + t * (v - u)));
    out
}

/// [§12.3.10 Description 2f iv 6] The linear curve of points `coords`
/// broken at `point`, which lies on its segment `k`: the part before and
/// the part after, each holding the point.
pub(super) fn cut(coords: &Coords, dims: Dims, k: usize, point: &Geom) -> (Coords, Coords) {
    let p = point_xy(point).unwrap_or_default();
    let points: Vec<&[f64]> = coords.chunks_exact(dims.stride()).collect();
    let (a, b) = (points.get(k).copied().unwrap_or_default(), points.get(k.saturating_add(1)).copied().unwrap_or_default());
    let at = vertex(p, (a, b), point, dims);
    let mut first: Coords = points.iter().take(k.saturating_add(1)).flat_map(|q| q.iter().copied()).collect();
    if !same(xy(a), p) {
        first.extend(&at);
    }
    let mut second = at;
    let rest = points.iter().skip(k.saturating_add(1)).skip_while(|q| same(xy(q), p));
    second.extend(rest.flat_map(|q| q.iter().copied()));
    (first, second)
}

/// The points of `c`, of `dims`, in reverse order.
pub(super) fn reversed(c: &Coords, dims: Dims) -> Coords {
    let points: Vec<&[f64]> = c.chunks_exact(dims.stride()).collect();
    points.into_iter().rev().flatten().copied().collect()
}

/// [§12.3.12 Description 2f v 7] `a` connected to `b`: `b`'s first point
/// left out where it is `a`'s last in x, y and z, else a segment between
/// them.
pub(super) fn joined(a: Coords, b: &Coords, dims: Dims) -> Coords {
    let stride = dims.stride();
    let last = a.len().checked_sub(stride).and_then(|k| a.get(k..));
    let meets = match (last, b.get(..stride)) {
        (Some(l), Some(f)) => l.iter().zip(f).take(dims.planar()).all(|(x, y)| x.to_bits() == y.to_bits()),
        (None, _) | (_, None) => false,
    };
    let mut out = a;
    out.extend(b.iter().skip(if meets { stride } else { 0 }));
    out
}

/// A linestring of `points` of the system and dimensions of `like`.
///
/// # Errors
///
/// As `Geom::new`.
pub(super) fn linestring(points: Coords, like: &Geom) -> Result<Geom> {
    Geom::new(like.srid, like.dims, Body::Curve(Curve::LineString(points)))
}
