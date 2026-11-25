//! Linear approximation of curves, ISO/IEC 13249-3:2016 (DIS, SC32
//! N2593) §7.1.10: ST_CurveToLine's approximation is implementation
//! defined. Conic arcs are cut at equal angles so the chord's distance
//! from the arc, r (1 - cos(θ/2)), stays within the tolerance; other
//! pieces are halved where their middle strays from the chord. Each
//! point's m is interpolated by length (§7.1.10 D4).

use super::geodesy::Geodesy;
use super::numeric::{HALF, V3, axpy, cross, dot, norm, sub};
use super::path::Path;
use super::piece::{Piece, Shape};
use super::types::Coord;
use crate::error::Result;
use std::f64::consts::PI;

/// How closely a linearization follows its curve.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tolerance {
    /// The largest distance between a chord and its curve, when given.
    pub deviation: Option<f64>,
    /// Otherwise, that distance as a fraction of the piece's size: its
    /// radius for an arc, the span of its points otherwise.
    pub relative: f64,
    /// The most segments one piece is cut into.
    pub max_segments: usize,
}

/// ST_CurveToLine's chord error: 10⁻³ of the piece's size, an
/// implementation-defined choice (§7.1.10 D2b) that keeps a circle under
/// 72 segments.
pub const RELATIVE: f64 = 1e-3;
/// The most segments per piece by default: 2¹⁶.
pub const MAX_SEGMENTS: usize = 65_536;
/// Pieces other than arcs start cut in this many parts, so no turn
/// between two samples goes unseen.
const INITIAL_PARTS: u32 = 16;
/// Halving stops at this depth below the initial parts.
const MAX_HALVINGS: u32 = 24;

impl Tolerance {
    /// The tolerance of ST_CurveToLine.
    pub const DEFAULT: Tolerance = Tolerance { deviation: None, relative: RELATIVE, max_segments: MAX_SEGMENTS };

    fn chord_error(&self, size: f64) -> f64 {
        self.deviation.unwrap_or(self.relative * size).abs()
    }
}

/// The vertices of a line string approximating `path`, its first and
/// last the path's own.
///
/// # Errors
///
/// As the geodesic problem's.
pub fn linearize<G: Geodesy>(path: &Path, tolerance: &Tolerance, geo: &G) -> Result<Vec<Coord>> {
    let mut out = vec![path.start];
    for piece in &path.pieces {
        let samples = parameters(piece, tolerance, geo)?;
        for p in samples.iter().skip(1) {
            out.push(Coord::at(piece.point(*p, geo)?, piece.measure_at(*p, geo)?));
        }
    }
    if let Some(last) = out.last_mut() {
        *last = path.end;
    }
    Ok(out)
}

/// The parameters at which `piece` is sampled, its range's ends first
/// and last.
///
/// # Errors
///
/// As the geodesic problem's.
pub fn parameters<G: Geodesy>(piece: &Piece, tolerance: &Tolerance, geo: &G) -> Result<Vec<f64>> {
    let (lo, hi) = piece.range(geo)?;
    match &piece.shape {
        Shape::Line { .. } => Ok(vec![lo, hi]),
        Shape::Conic { u, v, .. } => Ok(conic(*u, *v, lo, hi, tolerance)),
        Shape::Spline(_) | Shape::Clothoid { .. } | Shape::Spiral { .. } | Shape::Geodesic { .. } => {
            adaptive(piece, lo, hi, tolerance, geo)
        }
    }
}

/// Equal angle steps; an ellipse is held to the bound of its larger axis.
fn conic(u: V3, v: V3, lo: f64, hi: f64, tolerance: &Tolerance) -> Vec<f64> {
    let radius = norm(u).max(norm(v));
    let error = tolerance.chord_error(radius);
    let step = if error >= radius || radius <= 0.0_f64 { PI } else { 2.0_f64 * (1.0 - error / radius).acos() };
    let parts = count((hi - lo).abs() / step, tolerance.max_segments);
    let total = f64::from(u32::try_from(parts).unwrap_or(u32::MAX));
    (0..=parts).map(|k| f64::from(u32::try_from(k).unwrap_or(u32::MAX)) / total).map(|t| super::numeric::lerp(lo, hi, t)).collect()
}

/// The ceiling of `x`, at least one and at most `most`, and `most` only
/// of an `x` reaching it, whatever `most` is (RULES.md D1166).
pub fn count(x: f64, most: usize) -> usize {
    let cap = crate::stats::parts::count(u64::try_from(most).unwrap_or(u64::MAX));
    if x.is_nan() || x <= 1.0_f64 {
        return 1;
    }
    if x >= cap {
        return most.max(1);
    }
    format!("{:.0}", x.ceil()).parse::<usize>().map_or(1, |n| n.clamp(1, most.max(1)))
}

/// A sample: parameter and point.
type Sample = (f64, V3);

/// Initial equal parts, each halved while its middle strays from its
/// chord by more than the tolerance.
fn adaptive<G: Geodesy>(piece: &Piece, lo: f64, hi: f64, tolerance: &Tolerance, geo: &G) -> Result<Vec<f64>> {
    let mut first = Vec::new();
    for k in 0..=INITIAL_PARTS {
        let p = super::numeric::lerp(lo, hi, f64::from(k) / f64::from(INITIAL_PARTS));
        first.push((p, piece.point(p, geo)?));
    }
    let mut cutter = Cutter { piece, geo, error: tolerance.chord_error(extent(&first)), budget: tolerance.max_segments, out: vec![lo] };
    for pair in first.windows(2) {
        if let [a, b] = pair {
            cutter.halve((*a, *b), MAX_HALVINGS)?;
        }
    }
    Ok(cutter.out)
}

/// The state of one piece's adaptive sampling.
struct Cutter<'a, G> {
    piece: &'a Piece,
    geo: &'a G,
    error: f64,
    budget: usize,
    out: Vec<f64>,
}

impl<G: Geodesy> Cutter<'_, G> {
    /// Appends the parameters after `a` up to `b`, halving while the
    /// middle strays more than the error and depth and budget remain.
    fn halve(&mut self, ends: (Sample, Sample), depth: u32) -> Result<()> {
        let ((pa, a), (pb, b)) = ends;
        let pm = HALF * (pa + pb);
        let m = self.piece.point(pm, self.geo)?;
        if depth > 0 && self.budget > 1 && stray(a, b, m) > self.error {
            self.budget = self.budget.saturating_sub(1);
            let next = depth.saturating_sub(1);
            self.halve(((pa, a), (pm, m)), next)?;
            return self.halve(((pm, m), (pb, b)), next);
        }
        self.out.push(pb);
        Ok(())
    }
}

/// The distance from `m` to the segment a b.
pub fn stray(a: V3, b: V3, m: V3) -> f64 {
    let ab = sub(b, a);
    let am = sub(m, a);
    let length = dot(ab, ab);
    if length <= 0.0_f64 {
        return norm(am);
    }
    let t = (dot(am, ab) / length).clamp(0.0, 1.0);
    if t <= 0.0_f64 || t >= 1.0_f64 {
        return norm(sub(m, axpy(a, t, ab)));
    }
    norm(cross(ab, am)) / length.sqrt()
}

/// The diagonal of the samples' bounding box, one when they coincide.
fn extent(samples: &[Sample]) -> f64 {
    let mut low = [f64::INFINITY; 3];
    let mut high = [f64::NEG_INFINITY; 3];
    for (_, p) in samples {
        for ((l, h), v) in low.iter_mut().zip(high.iter_mut()).zip(p) {
            *l = l.min(*v);
            *h = h.max(*v);
        }
    }
    let size = norm(sub(high, low));
    if size.is_finite() && size > 0.0 { size } else { 1.0 }
}

#[cfg(test)]
mod tests;
