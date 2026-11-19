//! The observers of ST_Curve and its subtypes, ISO/IEC 13249-3:2016 (DIS,
//! SC32 N2593) §7.1 to §7.10: length, end points, closedness, rings,
//! linearization, points at a distance, distances to a point and
//! perpendicular projections, and the point and segment accessors of the
//! curves given by points. Empty curves give the null value, here none.

use super::fault::{Fault, fault};
use super::geodesy::Geodesy;
use super::linearize::{Tolerance, linearize, parameters};
use super::numeric::{HALF, POINT_TOLERANCE, Sum, V3, dot, invert, norm, same, sub};
use super::path::{Path, path};
use super::piece::{Metric, Piece, Shape};
use super::types::{Coord, Curve};
use crate::error::Result;

/// A point counts as on a curve within 10⁻⁹ of the coordinates'
/// magnitude: the curve's point is computed, so this is looser than
/// [`POINT_TOLERANCE`] (an implementation-defined tolerance, §7.1.11
/// D2a iv).
pub const ON_CURVE: f64 = 1e-9;

/// The outcome of a positional accessor (ST_PointN, ST_SegmentN,
/// ST_CurveN).
#[derive(Clone, Debug, PartialEq)]
pub enum Position<T> {
    /// The value is the empty set: the result is null.
    Empty,
    /// The position is outside the value: the warning 01F01 invalid
    /// position is raised and the result is null.
    Invalid,
    /// The element at the position.
    Found(T),
}

/// ST_Length (`Metric::Plane`) or ST_3DLength (`Metric::Space`), §7.1.2,
/// §7.1.3.
///
/// # Errors
///
/// As [`path`]'s.
pub fn length<G: Geodesy>(curve: &Curve, metric: Metric, geo: &G) -> Result<Option<f64>> {
    path(curve, geo)?.map(|p| path_length(&p, metric, geo)).transpose()
}

/// The length of a path's pieces together.
///
/// # Errors
///
/// As the geodesic problem's.
pub fn path_length<G: Geodesy>(path: &Path, metric: Metric, geo: &G) -> Result<f64> {
    let mut total = Sum::default();
    for piece in &path.pieces {
        total.add(piece.length(metric, geo)?);
    }
    Ok(total.value())
}

/// ST_StartPoint, §7.1.4.
///
/// # Errors
///
/// As [`path`]'s.
pub fn start_point<G: Geodesy>(curve: &Curve, geo: &G) -> Result<Option<Coord>> {
    Ok(path(curve, geo)?.map(|p| p.start))
}

/// ST_EndPoint, §7.1.5.
///
/// # Errors
///
/// As [`path`]'s.
pub fn end_point<G: Geodesy>(curve: &Curve, geo: &G) -> Result<Option<Coord>> {
    Ok(path(curve, geo)?.map(|p| p.end))
}

/// ST_IsClosed (`Metric::Plane`) or ST_3DIsClosed, §7.1.6, §7.1.7: the
/// start point equals the end point; an empty curve is not closed.
///
/// # Errors
///
/// As [`path`]'s.
pub fn is_closed<G: Geodesy>(curve: &Curve, metric: Metric, geo: &G) -> Result<bool> {
    Ok(path(curve, geo)?.is_some_and(|p| closed(&p, metric)))
}

fn closed(path: &Path, metric: Metric) -> bool {
    let (a, b) = (path.start.space(), path.end.space());
    match metric {
        Metric::Plane => same(super::numeric::flat(a), super::numeric::flat(b)),
        Metric::Space => same(a, b),
    }
}

/// ST_IsSimple of a curve, tested on its linearization; the empty set
/// is simple.
///
/// # Errors
///
/// As [`path`]'s.
pub fn is_simple<G: Geodesy>(curve: &Curve, metric: Metric, geo: &G) -> Result<bool> {
    if let Curve::Circle(_) = curve {
        path(curve, geo)?;
        return Ok(true);
    }
    let Some(p) = path(curve, geo)? else {
        return Ok(true);
    };
    let points: Vec<V3> = linearize(&p, &Tolerance::DEFAULT, geo)?.into_iter().map(Coord::space).collect();
    Ok(super::simple::is_simple(&points, metric))
}

/// ST_IsRing (`Metric::Plane`) or ST_3DIsRing, §7.1.8, §7.1.9: closed and
/// simple. A circle is always a ring (§7.4.1 D9).
///
/// # Errors
///
/// As [`path`]'s.
pub fn is_ring<G: Geodesy>(curve: &Curve, metric: Metric, geo: &G) -> Result<bool> {
    if curve.is_empty() {
        return Ok(false);
    }
    Ok(is_closed(curve, metric, geo)? && is_simple(curve, metric, geo)?)
}

/// ST_CurveToLine, §7.1.10, at `tolerance`; the empty curve gives the
/// empty line string.
///
/// # Errors
///
/// As [`path`]'s.
pub fn curve_to_line<G: Geodesy>(curve: &Curve, tolerance: &Tolerance, geo: &G) -> Result<Vec<Coord>> {
    match path(curve, geo)? {
        Some(p) => linearize(&p, tolerance, geo),
        None => Ok(Vec::new()),
    }
}

/// ST_PointAtDistance (`Metric::Plane`) or ST_3DPtAtDistance, §7.1.13,
/// §7.1.14: the point `distance` along the curve, with its z and its m
/// interpolated by length.
///
/// # Errors
///
/// SQLSTATE 2FF79 when the curve is shorter than `distance`, 2FF02 for
/// a negative or not finite distance; otherwise as [`path`]'s.
pub fn point_at_distance<G: Geodesy>(curve: &Curve, distance: f64, metric: Metric, geo: &G) -> Result<Option<Coord>> {
    let Some(p) = path(curve, geo)? else {
        return Ok(None);
    };
    if !distance.is_finite() || distance < 0.0_f64 {
        return Err(fault(Fault::InvalidArgument, "§7.1.13", "the distance is not a nonnegative number"));
    }
    let total = path_length(&p, metric, geo)?;
    if distance > total {
        return Err(fault(Fault::DistanceTooLong, "§7.1.13", &format!("the curve is {total} long")));
    }
    let mut rest = distance;
    for piece in &p.pieces {
        let length = piece.length(metric, geo)?;
        if rest <= length {
            let at = piece.locate(rest, metric, geo)?;
            return Ok(Some(Coord::at(piece.point(at, geo)?, piece.measure_at(at, geo)?)));
        }
        rest -= length;
    }
    Ok(Some(p.end))
}

/// ST_DistanceToPoint (`Metric::Plane`) or ST_3DDistanceToPt, §7.1.11,
/// §7.1.12: the length from the start to the first place the curve
/// passes through `point`.
///
/// # Errors
///
/// SQLSTATE 2FF80 when the curve does not pass through the point;
/// otherwise as [`path`]'s.
pub fn distance_to_point<G: Geodesy>(curve: &Curve, point: Coord, metric: Metric, geo: &G) -> Result<Option<f64>> {
    let Some(p) = path(curve, geo)? else {
        return Ok(None);
    };
    let target = project(point.space(), metric);
    let tolerance = ON_CURVE * norm(target).max(1.0);
    let mut before = Sum::default();
    for piece in &p.pieces {
        for t in candidates(piece, target, metric, geo)? {
            if norm(sub(project(piece.point(t, geo)?, metric), target)) <= tolerance {
                return Ok(Some(before.value() + piece.length_to(t, metric, geo)?));
            }
        }
        before.add(piece.length(metric, geo)?);
    }
    Err(fault(Fault::PointNotOnCurve, "§7.1.11", "the curve does not pass through the point"))
}

fn project(p: V3, metric: Metric) -> V3 {
    match metric {
        Metric::Plane => super::numeric::flat(p),
        Metric::Space => p,
    }
}

/// The parameters of `piece` where its tangent is perpendicular to the
/// direction to `target`, plus its ends, in order.
fn candidates<G: Geodesy>(piece: &Piece, target: V3, metric: Metric, geo: &G) -> Result<Vec<f64>> {
    let (lo, hi) = piece.range(geo)?;
    let mut out = vec![lo];
    out.extend(feet(piece, target, metric, geo)?);
    out.push(hi);
    Ok(out)
}

/// The parameters strictly inside the piece's range where
/// (c(t) - target) · c'(t) changes sign, each refined by bisection.
fn feet<G: Geodesy>(piece: &Piece, target: V3, metric: Metric, geo: &G) -> Result<Vec<f64>> {
    let gap = |t: f64| -> Result<f64> {
        let offset = project(sub(piece.point(t, geo)?, target), metric);
        Ok(dot(offset, project(piece.velocity(t, geo)?, metric)))
    };
    let samples = parameters(piece, &Tolerance::DEFAULT, geo)?;
    let mut out = Vec::new();
    let mut failure = None;
    for pair in samples.windows(2) {
        let [a, b] = <[f64; 2]>::try_from(pair).unwrap_or_default();
        let (ga, gb) = (gap(a)?, gap(b)?);
        if (ga < 0.0_f64) == (gb < 0.0_f64) {
            continue;
        }
        let sign = if ga < 0.0_f64 { 1.0_f64 } else { -1.0_f64 };
        let t = invert(
            |u| match gap(super::numeric::lerp(a, b, u)) {
                Ok(g) => sign * g,
                Err(e) => {
                    failure = Some(e);
                    0.0_f64
                }
            },
            0.0,
            0.0,
            1.0,
        );
        out.push(super::numeric::lerp(a, b, t));
    }
    failure.map_or(Ok(out), Err)
}

/// A perpendicular projection of a point on a curve (§7.1.15).
#[derive(Clone, Debug, PartialEq)]
pub enum Foot {
    /// A single point.
    Point(Coord),
    /// A whole circular arc, start, middle and end: the point is its
    /// center.
    Arc([Coord; 3]),
}

/// ST_PerpPoints, §7.1.15: the point itself when the curve passes
/// through it, else every perpendicular projection, x and y only, with
/// m interpolated; none for an empty curve.
///
/// # Errors
///
/// As [`path`]'s.
pub fn perp_points<G: Geodesy>(curve: &Curve, point: Coord, geo: &G) -> Result<Option<Vec<Foot>>> {
    let Some(p) = path(curve, geo)? else {
        return Ok(None);
    };
    if distance_to_point(curve, point, Metric::Plane, geo).is_ok() {
        return Ok(Some(vec![Foot::Point(point)]));
    }
    let target = project(point.space(), Metric::Plane);
    let mut out = Vec::new();
    for piece in &p.pieces {
        if let Some(arc) = centered(piece, target, geo)? {
            out.push(Foot::Arc(arc));
            continue;
        }
        for t in feet(piece, target, Metric::Plane, geo)? {
            let foot = Coord::at(project(piece.point(t, geo)?, Metric::Plane), piece.measure_at(t, geo)?);
            if out.last().is_none_or(|last| *last != Foot::Point(foot)) {
                out.push(Foot::Point(foot));
            }
        }
    }
    Ok(Some(out))
}

/// The piece as an arc when it is circular in the plane and centered on
/// `target`: every point of it is then a perpendicular projection.
fn centered<G: Geodesy>(piece: &Piece, target: V3, geo: &G) -> Result<Option<[Coord; 3]>> {
    let Shape::Conic { o, u, v, a0, a1 } = &piece.shape else {
        return Ok(None);
    };
    let (ru, rv) = (super::numeric::norm2(*u), super::numeric::norm2(*v));
    let round = (ru - rv).abs() <= POINT_TOLERANCE * ru.max(rv)
        && dot(project(*u, Metric::Plane), project(*v, Metric::Plane)).abs() <= POINT_TOLERANCE * ru * rv;
    if !round || !same(project(*o, Metric::Plane), target) {
        return Ok(None);
    }
    let at = |t: f64| -> Result<Coord> { Ok(Coord::at(project(piece.point(t, geo)?, Metric::Plane), piece.measure_at(t, geo)?)) };
    Ok(Some([at(*a0)?, at(HALF * (a0 + a1))?, at(*a1)?]))
}

/// The points of a curve given by points (§7.2.3, §7.3.3, §7.4.3,
/// §7.5.3), none for the other types.
pub fn points(curve: &Curve) -> Option<&[Coord]> {
    match curve {
        Curve::LineString(p) | Curve::CircularString(p) | Curve::Circle(p) | Curve::GeodesicString(p) => Some(p),
        Curve::Elliptical(_) | Curve::Nurbs(_) | Curve::Clothoid(_) | Curve::Spiral(_) | Curve::Compound(_) => None,
    }
}

/// The element at 1-based `position`, ST_PointN (§7.2.5, §7.3.5) and
/// ST_CurveN of a compound curve.
pub fn nth<T: Clone>(items: &[T], position: i64) -> Position<T> {
    if items.is_empty() {
        return Position::Empty;
    }
    let found = usize::try_from(position).ok().and_then(|p| p.checked_sub(1)).and_then(|i| items.get(i));
    found.map_or(Position::Invalid, |item| Position::Found(item.clone()))
}

/// ST_NumSegments of a circular string, §7.3.6: (points - 1) / 2.
pub fn num_segments(points: &[Coord]) -> Option<usize> {
    (!points.is_empty()).then(|| points.len().saturating_sub(1).checked_div(2).unwrap_or_default())
}

/// ST_SegmentN of a circular string, §7.3.7: the three points of arc
/// `position`, counted from one.
pub fn segment_n(points: &[Coord], position: i64) -> Position<Vec<Coord>> {
    let segments = num_segments(points).unwrap_or_default();
    match nth(&vec![(); segments], position) {
        Position::Empty => Position::Empty,
        Position::Invalid => Position::Invalid,
        Position::Found(()) => {
            let first = usize::try_from(position).unwrap_or_default().saturating_sub(1).saturating_mul(2);
            Position::Found(points.iter().skip(first).take(3).copied().collect())
        }
    }
}

/// ST_MidPointRep, §7.3.8: the start point, then each arc's middle and
/// end points.
pub fn mid_point_rep(points: &[Coord]) -> Option<Vec<Coord>> {
    let mut out = vec![*points.first()?];
    for arc in points.windows(3).step_by(2) {
        if let [a, b, c] = arc {
            out.push(super::circular::midpoint([*a, *b, *c]));
            out.push(*c);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests;
