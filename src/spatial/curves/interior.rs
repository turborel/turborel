//! A point inside a curve polygon, ISO/IEC 13249-3:2016 (DIS, SC32 N2593)
//! §8.1.8 and §8.1.9. A scan line is drawn midway across the widest band
//! between the rings' critical heights (vertices, arcs' turning points,
//! other pieces' samples), so it meets no ring tangentially; its
//! crossings are exact for segments and arcs and bisected for the
//! rest, and the middle of the widest inside run is the answer. In
//! space the scan runs in the coordinate plane the polygon faces most,
//! and the point is lifted back onto the polygon's plane.

use super::geodesy::Geodesy;
use super::linearize::{Tolerance, parameters};
use super::numeric::{HALF, V3, invert, lerp};
use super::path::Path;
use super::piece::{Metric, Piece, Shape};
use super::surface::{normal, rings};
use super::types::{Coord, Curve};
use crate::error::Result;
use std::f64::consts::{PI, TAU};

/// The coordinate plane of the scan: the axes scanned along (`along`),
/// across (`across`) and dropped (`dropped`).
#[derive(Clone, Copy, Debug)]
struct Frame {
    along: usize,
    across: usize,
    dropped: usize,
}

impl Frame {
    fn of(n: V3) -> Frame {
        let [x, y, z] = n.map(f64::abs);
        if z >= x && z >= y {
            Frame { along: 0, across: 1, dropped: 2 }
        } else if y >= x {
            Frame { along: 2, across: 0, dropped: 1 }
        } else {
            Frame { along: 1, across: 2, dropped: 0 }
        }
    }

    fn get(axis: usize, v: V3) -> f64 {
        v.get(axis).copied().unwrap_or_default()
    }

    fn across(self, v: V3) -> f64 {
        Frame::get(self.across, v)
    }

    fn along(self, v: V3) -> f64 {
        Frame::get(self.along, v)
    }
}

/// ST_PointOnSurface (`Metric::Plane`, z zero) or ST_3DPointOnSurf,
/// §8.1.8, §8.1.9, of a curve polygon; none when it is empty or encloses
/// nothing.
///
/// # Errors
///
/// As [`rings`]'s.
pub fn point_on_surface<G: Geodesy>(curves: &[Curve], metric: Metric, geo: &G) -> Result<Option<Coord>> {
    let Some(paths) = rings(curves, geo)? else {
        return Ok(None);
    };
    let Some(exterior) = paths.first() else {
        return Ok(None);
    };
    let n = normal(exterior, metric, geo)?;
    let frame = Frame::of(n);
    let Some(level) = scan_level(&paths, frame, geo)? else {
        return Ok(None);
    };
    let mut crossings = Vec::new();
    for ring in &paths {
        for piece in &ring.pieces {
            crossings.extend(crosses(piece, frame, level, geo)?);
        }
    }
    crossings.sort_by(f64::total_cmp);
    let (runs, _) = crossings.as_chunks::<2>();
    let best = runs.iter().map(|[a, b]| (*a, *b)).max_by(|(a, b), (c, d)| (b - a).total_cmp(&(d - c)));
    let Some((a, b)) = best else {
        return Ok(None);
    };
    let mut point = [0.0_f64; 3];
    set(&mut point, frame.along, HALF * (a + b));
    set(&mut point, frame.across, level);
    if metric == Metric::Space {
        lift(&mut point, exterior.start.space(), n, frame);
    }
    Ok(Some(Coord::at(point, 0.0)))
}

fn set(v: &mut V3, axis: usize, value: f64) {
    if let Some(slot) = v.get_mut(axis) {
        *slot = value;
    }
}

/// Moves the dropped coordinate onto the plane through `origin` normal
/// to `n`.
fn lift(point: &mut V3, origin: V3, n: V3, frame: Frame) {
    let part = |axis: usize| Frame::get(axis, n) * (Frame::get(axis, *point) - Frame::get(axis, origin));
    let drop = Frame::get(frame.dropped, n);
    let value = Frame::get(frame.dropped, origin) - (part(frame.along) + part(frame.across)) / drop;
    set(point, frame.dropped, value);
}

/// The middle of the widest band between consecutive critical heights.
fn scan_level<G: Geodesy>(paths: &[Path], frame: Frame, geo: &G) -> Result<Option<f64>> {
    let mut heights = Vec::new();
    for ring in paths {
        for piece in &ring.pieces {
            critical(piece, frame, geo, &mut heights)?;
        }
    }
    heights.sort_by(f64::total_cmp);
    let widest = heights
        .windows(2)
        .filter_map(|w| if let [a, b] = w { Some((*a, *b)) } else { None })
        .max_by(|(a, b), (c, d)| (b - a).total_cmp(&(d - c)));
    Ok(widest.filter(|(a, b)| b > a).map(|(a, b)| HALF * (a + b)))
}

/// Appends the heights where `piece` may turn back across the scan.
fn critical<G: Geodesy>(piece: &Piece, frame: Frame, geo: &G, out: &mut Vec<f64>) -> Result<()> {
    let (lo, hi) = piece.range(geo)?;
    match &piece.shape {
        Shape::Line { a, b } => out.extend([frame.across(*a), frame.across(*b)]),
        Shape::Conic { u, v, .. } => {
            out.extend([frame.across(piece.point(lo, geo)?), frame.across(piece.point(hi, geo)?)]);
            let turn = frame.across(*v).atan2(frame.across(*u));
            for t in conic_angles(turn, lo, hi).into_iter().chain(conic_angles(turn + PI, lo, hi)) {
                out.push(frame.across(piece.point(t, geo)?));
            }
        }
        Shape::Spline(_) | Shape::Clothoid { .. } | Shape::Spiral { .. } | Shape::Geodesic { .. } => {
            for t in parameters(piece, &Tolerance::DEFAULT, geo)? {
                out.push(frame.across(piece.point(t, geo)?));
            }
        }
    }
    Ok(())
}

/// The angles congruent to `angle` modulo 2π within [lo, hi] either way.
fn conic_angles(angle: f64, lo: f64, hi: f64) -> Vec<f64> {
    let (low, high) = (lo.min(hi), lo.max(hi));
    let mut t = angle - TAU * ((angle - low) / TAU).floor();
    let mut out = Vec::new();
    while t <= high {
        out.push(t);
        t += TAU;
    }
    out
}

/// The scan coordinates where `piece` crosses height `level`.
fn crosses<G: Geodesy>(piece: &Piece, frame: Frame, level: f64, geo: &G) -> Result<Vec<f64>> {
    let (lo, hi) = piece.range(geo)?;
    let params = match &piece.shape {
        Shape::Line { a, b } => {
            let (ya, yb) = (frame.across(*a), frame.across(*b));
            if (ya < level) == (yb < level) { Vec::new() } else { vec![(level - ya) / (yb - ya)] }
        }
        Shape::Conic { o, u, v, .. } => {
            let (a, b, c) = (frame.across(*u), frame.across(*v), level - frame.across(*o));
            let radius = a.hypot(b);
            if radius <= 0.0_f64 || c.abs() > radius {
                Vec::new()
            } else {
                let (phase, spread) = (b.atan2(a), (c / radius).acos());
                let mut found = conic_angles(phase + spread, lo, hi);
                found.extend(conic_angles(phase - spread, lo, hi));
                found
            }
        }
        Shape::Spline(_) | Shape::Clothoid { .. } | Shape::Spiral { .. } | Shape::Geodesic { .. } => bisected(piece, frame, level, geo)?,
    };
    let mut out = Vec::with_capacity(params.len());
    for t in params {
        out.push(frame.along(piece.point(t, geo)?));
    }
    Ok(out)
}

fn bisected<G: Geodesy>(piece: &Piece, frame: Frame, level: f64, geo: &G) -> Result<Vec<f64>> {
    let samples = parameters(piece, &Tolerance::DEFAULT, geo)?;
    let mut out = Vec::new();
    for pair in samples.windows(2) {
        let [a, b] = <[f64; 2]>::try_from(pair).unwrap_or_default();
        let (ya, yb) = (frame.across(piece.point(a, geo)?), frame.across(piece.point(b, geo)?));
        if (ya < level) == (yb < level) {
            continue;
        }
        let sign = if ya < yb { 1.0_f64 } else { -1.0_f64 };
        let height = |t: f64| piece.point(lerp(a, b, t), geo).map_or(level, |p| frame.across(p));
        out.push(lerp(a, b, invert(|t| sign * (height(t) - level), 0.0, 0.0, 1.0)));
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
