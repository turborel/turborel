//! Self-intersection of polylines, for ST_IsSimple, ST_IsRing and
//! ST_3DIsRing of curves, ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.1.8,
//! §7.1.9. A curve is tested on its linearization; segments meet when
//! they come within the point tolerance of each other. Neighbours may
//! share their common vertex and nothing more, and a closed polyline's
//! last segment neighbours its first.

use super::numeric::{POINT_TOLERANCE, V3, axpy, dot, norm, sub};
use super::piece::Metric;
use std::cmp::Ordering;

/// Whether the polyline through `points` crosses or touches itself;
/// a closed one (first point equal to last) may meet only there.
pub fn is_simple(points: &[V3], metric: Metric) -> bool {
    let points = distinct(points, metric);
    let count = points.len().saturating_sub(1);
    if count < 2 {
        return true;
    }
    let closed = points.first().zip(points.last()).is_some_and(|(a, b)| same(*a, *b, metric));
    let tolerance = POINT_TOLERANCE * scale(&points);
    let segments: Vec<(V3, V3)> = points.windows(2).filter_map(|w| if let [a, b] = w { Some((*a, *b)) } else { None }).collect();
    let mut order: Vec<usize> = (0..count).collect();
    order.sort_by(|i, j| low_x(&segments, *i).total_cmp(&low_x(&segments, *j)).then(Ordering::Equal));
    let mut active: Vec<usize> = Vec::new();
    for i in order {
        let start = low_x(&segments, i);
        active.retain(|j| high_x(&segments, *j) + tolerance >= start);
        let pair = Pair { segments: &segments, count, closed, tolerance };
        if active.iter().any(|j| pair.conflict(i, *j)) {
            return false;
        }
        active.push(i);
    }
    true
}

/// The points with each run of equal ones kept once; a closed ring stays
/// closed.
fn distinct(points: &[V3], metric: Metric) -> Vec<V3> {
    let mut out: Vec<V3> = Vec::with_capacity(points.len());
    for p in points {
        let flat = project(*p, metric);
        if out.last().is_none_or(|q| !same(*q, flat, metric)) {
            out.push(flat);
        }
    }
    out
}

fn project(p: V3, metric: Metric) -> V3 {
    match metric {
        Metric::Plane => super::numeric::flat(p),
        Metric::Space => p,
    }
}

fn same(a: V3, b: V3, metric: Metric) -> bool {
    super::numeric::same(project(a, metric), project(b, metric))
}

fn scale(points: &[V3]) -> f64 {
    points.iter().flat_map(|p| p.iter()).fold(1.0_f64, |m, v| m.max(v.abs()))
}

fn low_x(segments: &[(V3, V3)], i: usize) -> f64 {
    segments.get(i).map_or(0.0, |([ax, ..], [bx, ..])| ax.min(*bx))
}

fn high_x(segments: &[(V3, V3)], i: usize) -> f64 {
    segments.get(i).map_or(0.0, |([ax, ..], [bx, ..])| ax.max(*bx))
}

/// The context of a pair test.
struct Pair<'a> {
    segments: &'a [(V3, V3)],
    count: usize,
    closed: bool,
    tolerance: f64,
}

impl Pair<'_> {
    /// Whether segments i and j meet where they should not.
    fn conflict(&self, i: usize, j: usize) -> bool {
        let (Some(&one), Some(&other)) = (self.segments.get(i), self.segments.get(j)) else {
            return false;
        };
        let (first, second) = (i.min(j), i.max(j));
        let last = self.count.saturating_sub(1);
        let (early, late) = if i < j { (one, other) } else { (other, one) };
        if second == first.saturating_add(1) {
            // Consecutive: they share the later one's start and may only turn there.
            return self.near(late.1, early.0, early.1) || self.near(early.0, late.0, late.1);
        }
        if self.closed && first == 0 && second == last {
            // The ring's first and last: they share its start.
            return self.near(early.1, late.0, late.1) || self.near(late.0, early.0, early.1);
        }
        segment_distance(one, other) <= self.tolerance
    }

    /// Whether `p` lies on the segment a b, other than at b's end that
    /// the neighbours share.
    fn near(&self, point: V3, from: V3, to: V3) -> bool {
        super::linearize::stray(from, to, point) <= self.tolerance
    }
}

/// The least distance between segments a b and c d.
pub fn segment_distance(first: (V3, V3), second: (V3, V3)) -> f64 {
    let ((p0, p1), (q0, q1)) = (first, second);
    let (along, across, between) = (sub(p1, p0), sub(q1, q0), sub(p0, q0));
    let (uu, uv, vv) = (dot(along, along), dot(along, across), dot(across, across));
    let (uw, vw) = (dot(along, between), dot(across, between));
    let denominator = uu.mul_add(vv, -(uv * uv));
    let mut on_first = if denominator > 0.0_f64 { (uv.mul_add(vw, -(vv * uw)) / denominator).clamp(0.0, 1.0) } else { 0.0_f64 };
    let mut on_second = if vv > 0.0_f64 { (uv.mul_add(on_first, vw) / vv).clamp(0.0, 1.0) } else { 0.0_f64 };
    if uu > 0.0_f64 {
        on_first = (uv.mul_add(on_second, -uw) / uu).clamp(0.0, 1.0);
        on_second = if vv > 0.0_f64 { (uv.mul_add(on_first, vw) / vv).clamp(0.0, 1.0) } else { 0.0_f64 };
    }
    norm(sub(axpy(p0, on_first, along), axpy(q0, on_second, across)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(points: &[(f64, f64)]) -> Vec<V3> {
        points.iter().map(|&(x, y)| [x, y, 0.0_f64]).collect()
    }

    /// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.1.8] A square
    /// ring is simple; a bow tie crosses itself; a spike that doubles back
    /// overlaps its neighbour; a ring touching its start mid-way is not
    /// simple; repeated points are ignored.
    #[test]
    fn polylines_and_rings() {
        let square = line(&[(0.0_f64, 0.0_f64), (1.0_f64, 0.0_f64), (1.0_f64, 1.0_f64), (0.0_f64, 1.0_f64), (0.0_f64, 0.0_f64)]);
        assert!(is_simple(&square, Metric::Plane));
        let bow = line(&[(0.0_f64, 0.0_f64), (1.0_f64, 1.0_f64), (1.0_f64, 0.0_f64), (0.0_f64, 1.0_f64), (0.0_f64, 0.0_f64)]);
        assert!(!is_simple(&bow, Metric::Plane));
        let spike = line(&[(0.0_f64, 0.0_f64), (2.0_f64, 0.0_f64), (1.0_f64, 0.0_f64)]);
        assert!(!is_simple(&spike, Metric::Plane));
        let touch = line(&[(0.0_f64, 0.0_f64), (2.0_f64, 0.0_f64), (2.0_f64, 2.0_f64), (0.0_f64, 0.0_f64), (-1.0_f64, 3.0_f64)]);
        assert!(!is_simple(&touch, Metric::Plane));
        let repeated = line(&[(0.0_f64, 0.0_f64), (1.0_f64, 0.0_f64), (1.0_f64, 0.0_f64), (1.0_f64, 1.0_f64)]);
        assert!(is_simple(&repeated, Metric::Plane));
        assert!(is_simple(&line(&[(0.0_f64, 0.0_f64), (1.0_f64, 0.0_f64)]), Metric::Plane));
        let back = line(&[(0.0_f64, 0.0_f64), (1.0_f64, 0.0_f64), (1.0_f64, 1.0_f64), (0.5_f64, 0.0_f64), (0.0_f64, 0.0_f64)]);
        assert!(!is_simple(&back, Metric::Plane));
    }

    /// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.1.9] Segments
    /// that cross in the plane but pass at different heights do not meet
    /// in space.
    #[test]
    fn heights_separate_in_space() {
        let points = [[0.0_f64, 0.0_f64, 0.0_f64], [2.0_f64, 2.0_f64, 0.0_f64], [2.0_f64, 0.0_f64, 1.0_f64], [0.0_f64, 2.0_f64, 1.0_f64]];
        assert!(!is_simple(&points, Metric::Plane));
        assert!(is_simple(&points, Metric::Space));
        let d = segment_distance(
            ([0.0_f64, 0.0_f64, 0.0_f64], [1.0_f64, 0.0_f64, 0.0_f64]),
            ([0.0_f64, 1.0_f64, 1.0_f64], [0.0_f64, 1.0_f64, 1.0_f64]),
        );
        assert!((d - 2.0_f64.sqrt()).abs() < 1e-15_f64);
    }
}
