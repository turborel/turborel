//! Area, perimeter and centroid of curve polygons, ISO/IEC 13249-3:2016
//! (DIS, SC32 N2593) §8.1.2 to §8.1.7, with arcs taken exactly. Each ring
//! is a fan of triangles from one reference point to its pieces' end
//! points, plus the region between each piece and its chord. A conic
//! piece's region is an affine image of a circular segment: area
//! det(U, V) (θ - sin θ)/2 and centroid 4 sin³(θ/2) / (3 (θ - sin θ))
//! out along the middle angle. Other pieces' regions are integrated.
//! Rings carry vector areas, so the 3D methods use the plane's normal.

use super::fault::{Fault, fault};
use super::geodesy::Geodesy;
use super::numeric::{HALF, Sum, V3, add, axpy, cross, dot, flat, integrate, norm, scale, sub, theta_minus_sin};
use super::observe::path_length;
use super::path::{Path, path};
use super::piece::{Metric, Piece, Shape};
use super::types::{Coord, Curve};
use crate::error::Result;

/// One third, for the centroids of triangles.
const THIRD: f64 = 1.0 / 3.0;
/// Two, the weight of the moving vertex in a sliver triangle's centroid.
const TWO: f64 = 2.0;
/// Four thirds, for the centroid of a circular segment.
const FOUR_THIRDS: f64 = 4.0 / 3.0;

/// The region a ring encloses, as signed area along `normal` and first
/// moment, both about the reference point.
#[derive(Clone, Copy, Debug, Default)]
pub struct Moments {
    /// The area, signed by the ring's turn about the normal.
    pub area: f64,
    /// The area-weighted centroid, relative to the reference point.
    pub moment: V3,
}

/// The paths of a polygon's rings, exterior first; none when empty.
///
/// # Errors
///
/// SQLSTATE 2FF74 when a ring is empty or not closed (§8.2.1), else as
/// [`path`]'s.
pub fn rings<G: Geodesy>(curves: &[Curve], geo: &G) -> Result<Option<Vec<Path>>> {
    if curves.first().is_none_or(Curve::is_empty) {
        return Ok(None);
    }
    let mut out = Vec::with_capacity(curves.len());
    for curve in curves {
        let ring = path(curve, geo)?.ok_or_else(|| fault(Fault::InvalidGeometry, "§8.2.1", "an interior ring is empty"))?;
        if !super::numeric::same(ring.start.space(), ring.end.space()) {
            return Err(fault(Fault::InvalidGeometry, "§8.2.1", "a ring is not closed"));
        }
        out.push(ring);
    }
    Ok(Some(out))
}

/// The vector area of a ring: half its integral of c × dc.
///
/// # Errors
///
/// As the geodesic problem's.
pub fn vector_area<G: Geodesy>(ring: &Path, reference: V3, metric: Metric, geo: &G) -> Result<V3> {
    let mut total = [Sum::default(); 3];
    for piece in &ring.pieces {
        for (sum, value) in total.iter_mut().zip(piece_area(piece, reference, metric, geo)?) {
            sum.add(value);
        }
    }
    Ok(total.map(Sum::value))
}

fn piece_area<G: Geodesy>(piece: &Piece, reference: V3, metric: Metric, geo: &G) -> Result<V3> {
    let (c0, c1) = ends(piece, reference, metric, geo)?;
    let triangle = scale(cross(c0, c1), HALF);
    let region = match &piece.shape {
        Shape::Line { .. } => [0.0_f64; 3],
        Shape::Conic { u, v, a0, a1, .. } => scale(cross(lay(*u, metric), lay(*v, metric)), HALF * theta_minus_sin(a1 - a0)),
        Shape::Spline(_) | Shape::Clothoid { .. } | Shape::Spiral { .. } | Shape::Geodesic { .. } => {
            let mut out = [0.0_f64; 3];
            for (axis, slot) in out.iter_mut().enumerate() {
                *slot = sweep(piece, reference, metric, geo, |c, dc| cross(sub(c, c0), dc).get(axis).copied().unwrap_or_default() * HALF)?;
            }
            out
        }
    };
    Ok(add(triangle, region))
}

/// The ring's area along unit `normal` and its first moment.
///
/// # Errors
///
/// As the geodesic problem's.
pub fn moments<G: Geodesy>(ring: &Path, reference: V3, normal: V3, metric: Metric, geo: &G) -> Result<Moments> {
    let mut area = Sum::default();
    let mut moment = [Sum::default(); 3];
    for piece in &ring.pieces {
        for (a, c) in piece_moments(piece, reference, normal, metric, geo)? {
            area.add(a);
            for (sum, value) in moment.iter_mut().zip(c) {
                sum.add(a * value);
            }
        }
    }
    Ok(Moments { area: area.value(), moment: moment.map(Sum::value) })
}

/// The piece's triangle and region, each as area and centroid; a
/// region with no area is given with its moment folded in.
fn piece_moments<G: Geodesy>(piece: &Piece, reference: V3, normal: V3, metric: Metric, geo: &G) -> Result<[(f64, V3); 2]> {
    let (c0, c1) = ends(piece, reference, metric, geo)?;
    let triangle = (HALF * dot(cross(c0, c1), normal), scale(add(c0, c1), THIRD));
    let region = match &piece.shape {
        Shape::Line { .. } => (0.0_f64, [0.0_f64; 3]),
        Shape::Conic { o, u, v, a0, a1 } => {
            conic_region(sub(lay(*o, metric), lay(reference, metric)), (lay(*u, metric), lay(*v, metric)), (*a0, *a1), normal)
        }
        Shape::Spline(_) | Shape::Clothoid { .. } | Shape::Spiral { .. } | Shape::Geodesic { .. } => {
            let area = sweep(piece, reference, metric, geo, |c, dc| HALF * dot(cross(sub(c, c0), dc), normal))?;
            let mut moment = [0.0_f64; 3];
            for (axis, slot) in moment.iter_mut().enumerate() {
                *slot = sweep(piece, reference, metric, geo, |c, dc| {
                    let weight = HALF * dot(cross(sub(c, c0), dc), normal);
                    weight * THIRD * axpy(c0, TWO, c).get(axis).copied().unwrap_or_default()
                })?;
            }
            (area, if area.abs() > 0.0_f64 { scale(moment, 1.0 / area) } else { [0.0_f64; 3] })
        }
    };
    Ok([triangle, region])
}

/// The circular segment of angles a0 to a1 mapped by o + U cos + V sin.
fn conic_region(o: V3, axes: (V3, V3), angles: (f64, f64), normal: V3) -> (f64, V3) {
    let ((u, v), (a0, a1)) = (axes, angles);
    let theta = a1 - a0;
    let excess = theta_minus_sin(theta);
    let area = HALF * dot(cross(u, v), normal) * excess;
    if excess.abs() <= 0.0_f64 {
        return (0.0, o);
    }
    let half = HALF * theta;
    let reach = FOUR_THIRDS * half.sin().powi(3) / excess;
    let (sin, cos) = HALF.mul_add(theta, a0).sin_cos();
    (area, axpy(axpy(o, reach * cos, u), reach * sin, v))
}

/// The piece's end points relative to the reference, in the metric's
/// space.
fn ends<G: Geodesy>(piece: &Piece, reference: V3, metric: Metric, geo: &G) -> Result<(V3, V3)> {
    let (lo, hi) = piece.range(geo)?;
    let r = lay(reference, metric);
    Ok((sub(lay(piece.point(lo, geo)?, metric), r), sub(lay(piece.point(hi, geo)?, metric), r)))
}

/// `v` in the metric's space: its z dropped for the plane.
fn lay(v: V3, metric: Metric) -> V3 {
    match metric {
        Metric::Plane => flat(v),
        Metric::Space => v,
    }
}

/// The integral over the piece of f(c, c'), c relative to the reference.
fn sweep<G: Geodesy>(piece: &Piece, reference: V3, metric: Metric, geo: &G, f: impl Fn(V3, V3) -> f64) -> Result<f64> {
    let (lo, hi) = piece.range(geo)?;
    let r = lay(reference, metric);
    let mut failure = None;
    let mut value = |t: f64| match (piece.point(t, geo), piece.velocity(t, geo)) {
        (Ok(c), Ok(dc)) => f(sub(lay(c, metric), r), lay(dc, metric)),
        (Err(e), _) | (_, Err(e)) => {
            failure = Some(e);
            0.0_f64
        }
    };
    let total = integrate(&mut value, lo, hi, 0.0);
    failure.map_or(Ok(total), Err)
}

/// The unit normal of the exterior ring's plane, +z in the plane metric
/// or for a ring without area.
///
/// # Errors
///
/// As the geodesic problem's.
pub fn normal<G: Geodesy>(exterior: &Path, metric: Metric, geo: &G) -> Result<V3> {
    let up = [0.0_f64, 0.0_f64, 1.0_f64];
    if metric == Metric::Plane {
        return Ok(up);
    }
    let area = vector_area(exterior, exterior.start.space(), metric, geo)?;
    let size = norm(area);
    Ok(if size > 0.0 { scale(area, 1.0 / size) } else { up })
}

/// ST_Area (`Metric::Plane`) or ST_3DArea of a curve polygon, §8.1.2,
/// §8.1.3: the exterior's area less the interiors'.
///
/// # Errors
///
/// As [`rings`]'s.
pub fn area<G: Geodesy>(curves: &[Curve], metric: Metric, geo: &G) -> Result<Option<f64>> {
    let Some(paths) = rings(curves, geo)? else {
        return Ok(None);
    };
    Ok(Some(polygon(&paths, metric, geo)?.area))
}

/// The polygon's area and first moment about its exterior's start,
/// exterior counted positive and interiors negative.
///
/// # Errors
///
/// As the geodesic problem's.
pub fn polygon<G: Geodesy>(paths: &[Path], metric: Metric, geo: &G) -> Result<Moments> {
    let Some(exterior) = paths.first() else {
        return Ok(Moments::default());
    };
    let reference = exterior.start.space();
    let up = normal(exterior, metric, geo)?;
    let mut area = Sum::default();
    let mut moment = [0.0_f64; 3];
    for (index, ring) in paths.iter().enumerate() {
        let m = moments(ring, reference, up, metric, geo)?;
        let sign = if (index == 0) == (m.area >= 0.0_f64) { 1.0_f64 } else { -1.0_f64 };
        area.add(sign * m.area);
        moment = axpy(moment, sign, m.moment);
    }
    Ok(Moments { area: area.value(), moment })
}

/// ST_Perimeter (`Metric::Plane`) or ST_3DPerimeter, §8.1.4, §8.1.5:
/// the length of every ring.
///
/// # Errors
///
/// As [`rings`]'s.
pub fn perimeter<G: Geodesy>(curves: &[Curve], metric: Metric, geo: &G) -> Result<Option<f64>> {
    let Some(paths) = rings(curves, geo)? else {
        return Ok(None);
    };
    let mut total = Sum::default();
    for ring in &paths {
        total.add(path_length(ring, metric, geo)?);
    }
    Ok(Some(total.value()))
}

/// ST_Centroid (`Metric::Plane`, z zero) or ST_3DCentroid, §8.1.6,
/// §8.1.7. A polygon without area has its boundary's centroid.
///
/// # Errors
///
/// As [`rings`]'s.
pub fn centroid<G: Geodesy>(curves: &[Curve], metric: Metric, geo: &G) -> Result<Option<Coord>> {
    let Some(paths) = rings(curves, geo)? else {
        return Ok(None);
    };
    let m = polygon(&paths, metric, geo)?;
    let reference = paths.first().map_or([0.0_f64; 3], |p| lay(p.start.space(), metric));
    if m.area.abs() > 0.0_f64 {
        return Ok(Some(Coord::at(axpy(reference, 1.0 / m.area, m.moment), 0.0)));
    }
    boundary_centroid(&paths, metric, geo).map(Some)
}

/// The length-weighted centroid of the rings' pieces' chords.
fn boundary_centroid<G: Geodesy>(paths: &[Path], metric: Metric, geo: &G) -> Result<Coord> {
    let (mut weight, mut sum) = (0.0_f64, [0.0_f64; 3]);
    for ring in paths {
        for piece in &ring.pieces {
            let (a, b) = ends(piece, [0.0_f64; 3], metric, geo)?;
            let length = norm(sub(b, a));
            weight += length;
            sum = axpy(sum, length * HALF, add(a, b));
        }
    }
    let first = paths.first().map_or([0.0_f64; 3], |p| lay(p.start.space(), metric));
    Ok(Coord::at(if weight > 0.0 { scale(sum, 1.0 / weight) } else { first }, 0.0))
}

#[cfg(test)]
mod tests;
