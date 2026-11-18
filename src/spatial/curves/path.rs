//! A curve as its sequence of pieces, ISO/IEC 13249-3:2016 (DIS, SC32
//! N2593) §7.2 to §7.10. Circular arc segments split at their middle
//! point, so m stays linear in length between the points that carry it
//! (§7.1.10 D4, an implementation-defined interpolation). Compound
//! curves must join end to start (§7.10.1 D7).

use super::arc::arc;
use super::fault::{Fault, fault};
use super::geodesy::Geodesy;
use super::numeric::{V3, same};
use super::nurbs::Spline;
use super::piece::{Piece, Shape};
use super::spiral::Transition;
use super::types::{Clothoid, Coord, Curve, Elliptical, Nurbs, Placement, Spiral};
use crate::error::Result;
use std::f64::consts::TAU;

/// A nonempty curve's pieces, with its end points.
#[derive(Clone, Debug)]
pub struct Path {
    /// The pieces in order.
    pub pieces: Vec<Piece>,
    /// The start point, with z and m.
    pub start: Coord,
    /// The end point, with z and m.
    pub end: Coord,
}

/// The pieces of `curve`, none when it is empty.
///
/// # Errors
///
/// SQLSTATE 2FF74 for a curve not well formed (a circular string's
/// point count, a NURBS's knots), 2FF77 or 2FF78 for a circle not of
/// three distinct points, 2FF86 for a placement without two directions,
/// 2FF11 for a compound curve whose members do not meet.
pub fn path<G: Geodesy>(curve: &Curve, geo: &G) -> Result<Option<Path>> {
    if curve.is_empty() {
        return Ok(None);
    }
    let pieces = match curve {
        Curve::LineString(points) => line_pieces(points),
        Curve::CircularString(points) => circular_pieces(points)?,
        Curve::Circle(points) => circle_pieces(points)?,
        Curve::GeodesicString(points) => geodesic_pieces(points),
        Curve::Elliptical(e) => e.as_ref().map(elliptical_piece).transpose()?.into_iter().collect(),
        Curve::Nurbs(n) => n.as_ref().map(nurbs_piece).transpose()?.into_iter().collect(),
        Curve::Clothoid(c) => c.as_ref().map(clothoid_piece).transpose()?.into_iter().collect(),
        Curve::Spiral(s) => s.as_ref().map(spiral_piece).transpose()?.into_iter().collect(),
        Curve::Compound(members) => return compound(members, geo).map(Some),
    };
    let (start, end) = if let Some(ends) = vertices(curve) {
        ends
    } else {
        let first = pieces.first().map(|p| end_of(p, false, geo)).transpose()?.unwrap_or_default();
        let last = pieces.last().map(|p| end_of(p, true, geo)).transpose()?.unwrap_or_default();
        (first, last)
    };
    Ok(Some(Path { pieces, start, end }))
}

/// The exact end vertices of a curve given by points; a circle ends
/// where it starts.
fn vertices(curve: &Curve) -> Option<(Coord, Coord)> {
    match curve {
        Curve::LineString(p) | Curve::CircularString(p) | Curve::GeodesicString(p) => Some((*p.first()?, *p.last()?)),
        Curve::Circle(p) => p.first().map(|first| (*first, *first)),
        Curve::Elliptical(_) | Curve::Nurbs(_) | Curve::Clothoid(_) | Curve::Spiral(_) | Curve::Compound(_) => None,
    }
}

/// A placed piece's start or end point.
fn end_of<G: Geodesy>(piece: &Piece, last: bool, geo: &G) -> Result<Coord> {
    let [m0, m1] = piece.m;
    let (lo, hi) = piece.range(geo)?;
    let (p, m) = if last { (hi, m1) } else { (lo, m0) };
    Ok(Coord::at(piece.point(p, geo)?, m))
}

fn line(a: Coord, b: Coord) -> Piece {
    Piece { shape: Shape::Line { a: a.space(), b: b.space() }, m: [a.m, b.m] }
}

fn line_pieces(points: &[Coord]) -> Vec<Piece> {
    points.windows(2).filter_map(|w| if let [a, b] = w { Some(line(*a, *b)) } else { None }).collect()
}

fn geodesic_pieces(points: &[Coord]) -> Vec<Piece> {
    points
        .windows(2)
        .filter_map(|w| if let [a, b] = w { Some(Piece { shape: Shape::Geodesic { a: *a, b: *b }, m: [a.m, b.m] }) } else { None })
        .collect()
}

/// Two conic pieces of the arc a → b → c, or the segment a → c when the
/// points are collinear (§7.3.1 D8b).
fn arc_pieces(points: [Coord; 3], out: &mut Vec<Piece>) -> Result<()> {
    let [a, mid_point, c] = points;
    if same(a.space(), c.space()) {
        return Err(fault(Fault::InvalidGeometry, "§7.3.1", "an arc's end point equals its start point (D6)"));
    }
    match arc(a.space(), mid_point.space(), c.space()) {
        Some((circle, mid, end)) => {
            let conic = conic_of(&circle);
            out.push(Piece { shape: conic(0.0_f64, mid), m: [a.m, mid_point.m] });
            out.push(Piece { shape: conic(mid, end), m: [mid_point.m, c.m] });
        }
        None => out.push(line(a, c)),
    }
    Ok(())
}

fn circular_pieces(points: &[Coord]) -> Result<Vec<Piece>> {
    let count = points.len();
    if count < super::types::Dims::Xyz.stride() || count.is_multiple_of(2) {
        return Err(fault(Fault::InvalidGeometry, "§7.3.1", "a circular string needs 2n + 1 points, n at least one (D7)"));
    }
    let mut out = Vec::with_capacity(count);
    let mut at = 0_usize;
    while let (Some(a), Some(b), Some(c)) = (points.get(at), points.get(at.saturating_add(1)), points.get(at.saturating_add(2))) {
        arc_pieces([*a, *b, *c], &mut out)?;
        at = at.saturating_add(2);
    }
    Ok(out)
}

/// The full circle through the three points, starting at the first
/// (§7.4.1 D6): three conic pieces, the last closing on the first point.
fn circle_pieces(points: &[Coord]) -> Result<Vec<Piece>> {
    let [first, second, third] =
        <[Coord; 3]>::try_from(points).map_err(|_| fault(Fault::ExactlyThreePoints, "§7.4.1", "a circle has three control points (D8)"))?;
    let (circle, mid, end) = arc(first.space(), second.space(), third.space())
        .ok_or_else(|| fault(Fault::Collinear, "§7.4.1", "a circle's control points are distinct and not collinear (D6)"))?;
    let conic = conic_of(&circle);
    Ok(vec![
        Piece { shape: conic(0.0_f64, mid), m: [first.m, second.m] },
        Piece { shape: conic(mid, end), m: [second.m, third.m] },
        Piece { shape: conic(end, TAU), m: [third.m, first.m] },
    ])
}

/// A maker of the circle's arcs between two angles.
fn conic_of(circle: &super::arc::Circle) -> impl Fn(f64, f64) -> Shape {
    let u = super::numeric::scale(circle.e1, circle.radius);
    let v = super::numeric::scale(circle.e2, circle.radius);
    let o = circle.center;
    move |a0, a1| Shape::Conic { o, u, v, a0, a1 }
}

/// The two reference directions a curve's placement needs (§7.6.1 D13,
/// §7.8.1 D12, §7.9.1 D13).
fn axes(placement: &Placement, clause: &str) -> Result<(V3, V3)> {
    match placement.directions.as_slice() {
        [d1, d2] => Ok((d1.space(), d2.space())),
        _ => Err(fault(Fault::VectorCount, clause, "the reference location needs exactly two reference directions")),
    }
}

fn measures(m: Option<[f64; 2]>) -> [f64; 2] {
    m.unwrap_or_default()
}

fn finite(values: &[f64], clause: &str) -> Result<()> {
    if values.iter().all(|v| v.is_finite()) {
        Ok(())
    } else {
        Err(fault(Fault::InvalidArgument, clause, "a curve parameter is not a finite number"))
    }
}

fn elliptical_piece(e: &Elliptical) -> Result<Piece> {
    let (d1, d2) = axes(&e.placement, "§7.6.1")?;
    finite(&[e.u_axis, e.v_axis, e.start, e.end], "§7.6.1")?;
    if e.u_axis <= 0.0_f64 || e.v_axis <= 0.0_f64 {
        return Err(fault(Fault::InvalidArgument, "§7.6.1", "an elliptical curve's axis lengths are positive"));
    }
    let o = e.placement.location.space();
    let (u, v) = (super::numeric::scale(d1, e.u_axis), super::numeric::scale(d2, e.v_axis));
    Ok(Piece { shape: Shape::Conic { o, u, v, a0: e.start, a1: e.end }, m: measures(e.measures) })
}

fn nurbs_piece(n: &Nurbs) -> Result<Piece> {
    Ok(Piece { shape: Shape::Spline(Box::new(Spline::new(n)?)), m: measures(n.measures) })
}

fn clothoid_piece(c: &Clothoid) -> Result<Piece> {
    axes(&c.placement, "§7.8.1")?;
    finite(&[c.scale, c.start, c.end], "§7.8.1")?;
    if c.scale.abs() <= 0.0_f64 {
        return Err(fault(Fault::InvalidArgument, "§7.8.1", "a clothoid's scale factor is not zero"));
    }
    let shape = Shape::Clothoid { placement: c.placement.clone(), scale: c.scale, s0: c.start, s1: c.end };
    Ok(Piece { shape, m: measures(c.measures) })
}

fn spiral_piece(s: &Spiral) -> Result<Piece> {
    axes(&s.placement, "§7.9.1")?;
    finite(&[s.length, s.start_curvature, s.end_curvature], "§7.9.1")?;
    if s.length < 0.0_f64 {
        return Err(fault(Fault::InvalidArgument, "§7.9.1", "a spiral's length is not negative"));
    }
    let transition = Transition { kind: s.kind, length: s.length, start: s.start_curvature, end: s.end_curvature };
    Ok(Piece { shape: Shape::Spiral { placement: s.placement.clone(), transition }, m: measures(s.measures) })
}

/// The members' pieces in turn; each member must start where the last
/// ended, x and y and z within the point tolerance (§7.10.1 D7).
fn compound<G: Geodesy>(members: &[Curve], geo: &G) -> Result<Path> {
    let mut pieces = Vec::new();
    let mut ends: Option<(Coord, Coord)> = None;
    for member in members {
        let part = path(member, geo)?.ok_or_else(|| fault(Fault::InvalidGeometry, "§7.10.1", "a compound curve's member is empty"))?;
        if let Some((_, end)) = ends
            && !same(end.space(), part.start.space())
        {
            return Err(fault(Fault::NonContiguousCurves, "§7.10.1", "a member does not start where the one before it ends (D7)"));
        }
        ends = Some((ends.map_or(part.start, |(start, _)| start), part.end));
        pieces.extend(part.pieces);
    }
    let (start, end) = ends.ok_or_else(|| fault(Fault::InvalidGeometry, "§7.10.1", "a compound curve has no members"))?;
    Ok(Path { pieces, start, end })
}

#[cfg(test)]
mod tests;
