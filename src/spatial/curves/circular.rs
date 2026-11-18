//! The other representations of circular arcs and circles, ISO/IEC
//! 13249-3:2016 (DIS, SC32 N2593) §7.3 and §7.4: control points with
//! bulges and bulge normals (§7.3.1 D13, ISO 19107 ArcByBulge, where the
//! arc's midpoint is the chord's midpoint plus bulge times normal),
//! center, radius and angles (D14, D15), center, radius and normal
//! (§7.4.2), and the observers that recover them from a single arc.
//! Every result is in the three-point form the types store.

use super::arc::{Circle, arc};
use super::fault::{Fault, fault};
use super::numeric::{HALF, V3, add, axpy, cross, dot, norm, same, scale, sub};
use super::types::Coord;
use crate::error::Result;
use std::f64::consts::TAU;

/// Normals farther than this cosine from perpendicular to their chord
/// are refused: 10⁻⁹, an implementation-defined tolerance.
const PERPENDICULAR: f64 = 1e-9;
/// A third of a turn, to place a circle's three control points.
const THIRD_TURN: f64 = TAU / 3.0;

/// The point array of ST_CircularString(points, bulges, normals),
/// §7.3.2 D16: each arc's middle point is (a + b)/2 + bulge × normal.
///
/// # Errors
///
/// SQLSTATE 2FF02 when the bulge and normal arrays are not one shorter
/// than the points, or a normal is not perpendicular to its chord;
/// 2FF74 when two consecutive control points are equal (§7.3.1 D6).
pub fn from_bulges(points: &[Coord], bulges: &[f64], normals: &[V3]) -> Result<Vec<Coord>> {
    let segments = points.len().saturating_sub(1);
    if points.is_empty() || bulges.len() != segments || normals.len() != segments {
        return Err(fault(Fault::InvalidArgument, "§7.3.1", "a bulge and a normal are needed for each pair of control points (D13b)"));
    }
    let mut out = Vec::with_capacity(points.len().saturating_mul(2));
    out.extend(points.first().copied());
    for ((pair, bulge), normal) in points.windows(2).zip(bulges).zip(normals) {
        if let [a, b] = pair {
            out.push(middle(*a, *b, *bulge, *normal)?);
            out.push(*b);
        }
    }
    Ok(out)
}

fn middle(a: Coord, b: Coord, bulge: f64, normal: V3) -> Result<Coord> {
    let chord = sub(b.space(), a.space());
    if same(a.space(), b.space()) {
        return Err(fault(Fault::InvalidGeometry, "§7.3.1", "an arc's end point equals its start point (D6)"));
    }
    let size = norm(normal) * norm(chord);
    if !bulge.is_finite() || !size.is_finite() || dot(normal, chord).abs() > PERPENDICULAR * size {
        return Err(fault(Fault::InvalidArgument, "§7.3.1", "a bulge normal is not perpendicular to its chord"));
    }
    let center = scale(add(a.space(), b.space()), HALF);
    Ok(Coord::at(axpy(center, bulge, normal), HALF * (a.m + b.m)))
}

/// The point array of ST_CircularString(center, radius, start, end),
/// angles in radians counterclockwise from the x axis, swept up from
/// start to end when start is the smaller and down otherwise (§7.3.1
/// D14, D15). The points have the center's z and m.
///
/// # Errors
///
/// SQLSTATE 2FF02 for a radius not positive, a sweep of zero or of a
/// full turn or more, or values that are not finite.
pub fn from_center(center: Coord, radius: f64, start: f64, end: f64) -> Result<Vec<Coord>> {
    let sweep = end - start;
    let finite = radius.is_finite() && start.is_finite() && sweep.is_finite();
    if !finite || radius <= 0.0_f64 || sweep.abs() <= 0.0_f64 || sweep.abs() >= TAU {
        return Err(fault(Fault::InvalidArgument, "§7.3.1", "the radius is positive and the arc sweeps less than a full turn (D14, D15)"));
    }
    let at = |angle: f64| {
        let (sin, cos) = angle.sin_cos();
        Coord { x: radius.mul_add(cos, center.x), y: radius.mul_add(sin, center.y), ..center }
    };
    Ok(vec![at(start), at(HALF.mul_add(sweep, start)), at(end)])
}

/// The control points of ST_Circle(center, radius, normal), §7.4.2
/// D16: a third of a turn apart, turning counterclockwise about the
/// normal. A circle of 2D points lies in the xy plane, counterclockwise
/// unless the normal's z is negative.
///
/// # Errors
///
/// SQLSTATE 2FF02 for a radius not positive, a zero normal, or values
/// that are not finite.
pub fn circle_from_center(center: Coord, radius: f64, normal: V3, space: bool) -> Result<Vec<Coord>> {
    let length = norm(normal);
    if !radius.is_finite() || radius <= 0.0_f64 || !length.is_finite() || length <= 0.0_f64 {
        return Err(fault(Fault::InvalidArgument, "§7.4.2", "the radius is positive and the normal is not zero"));
    }
    let [_, _, nz] = normal;
    let unit = if space { scale(normal, 1.0 / length) } else { [0.0_f64, 0.0_f64, if nz < 0.0_f64 { -1.0_f64 } else { 1.0_f64 }] };
    let e1 = perpendicular(unit);
    let e2 = cross(unit, e1);
    let circle = Circle { center: center.space(), radius, e1, e2 };
    Ok([0.0, THIRD_TURN, THIRD_TURN + THIRD_TURN].map(|angle| Coord::at(circle.point(angle), center.m)).to_vec())
}

/// A unit vector perpendicular to unit `n`, built on the axis `n` leans
/// on least.
fn perpendicular(n: V3) -> V3 {
    let [ax, ay, az] = n.map(f64::abs);
    let axis = if ax <= ay && ax <= az {
        [1.0_f64, 0.0_f64, 0.0_f64]
    } else if ay <= az {
        [0.0_f64, 1.0_f64, 0.0_f64]
    } else {
        [0.0_f64, 0.0_f64, 1.0_f64]
    };
    let across = cross(n, axis);
    scale(across, 1.0 / norm(across))
}

/// The only arc of a circular string, as its three points.
///
/// # Errors
///
/// SQLSTATE 2FF76 when the string has more than one arc (§7.3.9 to
/// §7.3.14), 2FF74 when it is not 2n + 1 points.
pub fn single(points: &[Coord]) -> Result<Option<[Coord; 3]>> {
    if points.is_empty() {
        return Ok(None);
    }
    if points.len() < 3 || points.len().is_multiple_of(2) {
        return Err(fault(Fault::InvalidGeometry, "§7.3.1", "a circular string needs 2n + 1 points (D7)"));
    }
    <[Coord; 3]>::try_from(points)
        .map(Some)
        .map_err(|_| fault(Fault::MultipleSegments, "§7.3.9", "the circular string has more than one arc"))
}

/// ST_Bulge and ST_BulgeNormal of an arc a → b → c: the offset of its
/// midpoint from its chord's. In 2D the normal is the chord's left unit
/// perpendicular and the bulge is signed, positive for an arc bulging
/// left; in 3D the normal points at the midpoint and the bulge is not
/// negative. A straight segment has bulge zero.
pub fn bulge(points: [Coord; 3], space: bool) -> (f64, V3) {
    let [a, _, c] = points;
    let chord = sub(c.space(), a.space());
    let middle = midpoint(points).space();
    let offset = sub(middle, scale(add(a.space(), c.space()), HALF));
    let [cx, cy, _] = chord;
    let left = scale([-cy, cx, 0.0_f64], 1.0 / cx.hypot(cy));
    if !space {
        return (dot(offset, left), left);
    }
    let size = norm(offset);
    if size > 0.0 { (size, scale(offset, 1.0 / size)) } else { (0.0, perpendicular(scale(chord, 1.0 / norm(chord)))) }
}

/// The point halfway along the arc, its m between the end points'; a
/// straight segment's middle.
pub fn midpoint(points: [Coord; 3]) -> Coord {
    let [a, b, c] = points;
    let m = HALF * (a.m + c.m);
    match arc(a.space(), b.space(), c.space()) {
        Some((circle, _, end)) => Coord::at(circle.point(HALF * end), m),
        None => Coord::at(scale(add(a.space(), c.space()), HALF), m),
    }
}

/// ST_Center and ST_Radius: the circle through the points, none for
/// collinear points, whose arc has no center (§7.3.1 D8b).
pub fn center(points: [Coord; 3]) -> Option<(Coord, f64)> {
    let [a, b, c] = points;
    arc(a.space(), b.space(), c.space()).map(|(circle, _, _)| (Coord::at(circle.center, 0.0), circle.radius))
}

/// ST_StartAngle and ST_EndAngle in radians, counterclockwise from the
/// x axis about the center, the end above the start for a
/// counterclockwise arc and below it otherwise (§7.3.1 D14, D15); none
/// for a straight segment.
pub fn angles(points: [Coord; 3]) -> Option<(f64, f64)> {
    let [a, b, c] = points;
    let (circle, _, end) = arc(a.space(), b.space(), c.space())?;
    let start = (a.y - circle.center[1]).atan2(a.x - circle.center[0]);
    let [_, _, nz] = circle.normal();
    let sweep = if nz < 0.0_f64 { -end } else { end };
    Some((start, start + sweep))
}

/// ST_Normal of a circle: the unit normal about which its points turn.
pub fn normal(points: [Coord; 3]) -> Option<V3> {
    let [a, b, c] = points;
    arc(a.space(), b.space(), c.space()).map(|(circle, _, _)| circle.normal())
}

#[cfg(test)]
mod tests;
