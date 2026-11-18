//! \[DESIGN\] The 3D distance of ISO/IEC 13249-3:2016 (DIS, SC32 N2593)
//! §5.1.42: zero for values that meet ([`super::ops::intersects`]), else
//! the least distance between their parts. Parts that do not meet are
//! nearest between a point and a part or between two segments, so the
//! least of those over points, segments, triangle edges and triangles
//! is the distance (Ericson, "Real-Time Collision Detection", 2005, §5.1).

use super::cells::{Cells, P3, sub};
use super::ops::intersects;
use crate::error::Result;
use crate::geometry::model::Geom;

/// The 3D distance between `first` and `second`; None if one is empty.
///
/// # Errors
///
/// As [`Cells::of`].
pub fn distance(first: &Geom, second: &Geom) -> Result<Option<f64>> {
    let (a, b) = (Cells::of(first)?, Cells::of(second)?);
    if a.is_empty() || b.is_empty() {
        return Ok(None);
    }
    if intersects(first, second)? {
        return Ok(Some(0.0_f64));
    }
    Ok(Some(between(&Parts::of(&a), &Parts::of(&b))))
}

/// A value's parts as points, segments and triangles.
struct Parts {
    points: Vec<P3>,
    segments: Vec<[P3; 2]>,
    triangles: Vec<[P3; 3]>,
}

impl Parts {
    fn of(cells: &Cells) -> Parts {
        let triangles: Vec<[P3; 3]> = cells.triangles.iter().chain(cells.solids.iter().flatten()).copied().collect();
        let mut segments = cells.segments.clone();
        segments.extend(triangles.iter().flat_map(|[a, b, c]| [[*a, *b], [*b, *c], [*c, *a]]));
        let mut points = cells.points.clone();
        points.extend(segments.iter().flatten());
        Parts { points, segments, triangles }
    }
}

/// The least distance between parts of `a` and of `b`.
fn between(a: &Parts, b: &Parts) -> f64 {
    let mut best = f64::INFINITY;
    for (one, other) in [(a, b), (b, a)] {
        for p in &one.points {
            for q in &other.points {
                best = best.min(length(sub(*p, *q)));
            }
            for s in &other.segments {
                best = best.min(length(sub(*p, on_segment(*p, *s))));
            }
            for t in &other.triangles {
                best = best.min(length(sub(*p, on_triangle(*p, *t))));
            }
        }
    }
    for s in &a.segments {
        for t in &b.segments {
            best = best.min(segments(*s, *t));
        }
    }
    best
}

/// The length of `v`.
pub fn length(v: P3) -> f64 {
    v[0].hypot(v[1]).hypot(v[2])
}

fn dot(u: P3, v: P3) -> f64 {
    u[0] * v[0] + u[1] * v[1] + u[2] * v[2]
}

fn along(a: P3, d: P3, t: f64) -> P3 {
    [a[0] + t * d[0], a[1] + t * d[1], a[2] + t * d[2]]
}

/// The point of segment `s` nearest `p`.
pub fn on_segment(p: P3, [a, b]: [P3; 2]) -> P3 {
    let d = sub(b, a);
    let squared = dot(d, d);
    if squared <= 0.0_f64 {
        return a;
    }
    along(a, d, (dot(sub(p, a), d) / squared).clamp(0.0_f64, 1.0_f64))
}

/// The point of triangle `corners` nearest `p` (Ericson §5.1.5).
pub fn on_triangle(p: P3, corners: [P3; 3]) -> P3 {
    let [first, second, third] = corners;
    let (ab, ac) = (sub(second, first), sub(third, first));
    let (d1, d2) = (dot(ab, sub(p, first)), dot(ac, sub(p, first)));
    if d1 <= 0.0_f64 && d2 <= 0.0_f64 {
        return first;
    }
    let (d3, d4) = (dot(ab, sub(p, second)), dot(ac, sub(p, second)));
    if d3 >= 0.0_f64 && d4 <= d3 {
        return second;
    }
    let (d5, d6) = (dot(ab, sub(p, third)), dot(ac, sub(p, third)));
    if d6 >= 0.0_f64 && d5 <= d6 {
        return third;
    }
    let weights = [d3 * d6 - d5 * d4, d5 * d2 - d1 * d6, d1 * d4 - d3 * d2];
    if weights.iter().any(|w| *w <= 0.0_f64) {
        let edges = [[first, second], [second, third], [third, first]].map(|edge| on_segment(p, edge));
        return edges.into_iter().min_by(|x, y| length(sub(p, *x)).total_cmp(&length(sub(p, *y)))).unwrap_or(first);
    }
    let total = weights.iter().sum::<f64>();
    let (toward_second, toward_third) = (weights[1] / total, weights[2] / total);
    let toward = along(first, ab, toward_second);
    along(toward, ac, toward_third)
}

/// The least distance between segments `one` and `other` (Ericson
/// §5.1.9).
pub fn segments(one: [P3; 2], other: [P3; 2]) -> f64 {
    let ([start, end], [from, to]) = (one, other);
    let (d1, d2, offset) = (sub(end, start), sub(to, from), sub(start, from));
    let (first_squared, second_squared, reach) = (dot(d1, d1), dot(d2, d2), dot(d2, offset));
    if first_squared <= 0.0_f64 || second_squared <= 0.0_f64 {
        let near = [on_segment(start, other), on_segment(from, one)];
        return length(sub(start, near[0])).min(length(sub(from, near[1])));
    }
    let (lead, cosine) = (dot(d1, offset), dot(d1, d2));
    let denominator = first_squared * second_squared - cosine * cosine;
    let mut s =
        if denominator > 0.0_f64 { ((cosine * reach - lead * second_squared) / denominator).clamp(0.0_f64, 1.0_f64) } else { 0.0_f64 };
    let mut t = (cosine * s + reach) / second_squared;
    if t < 0.0_f64 {
        t = 0.0_f64;
        s = (-lead / first_squared).clamp(0.0_f64, 1.0_f64);
    } else if t > 1.0_f64 {
        t = 1.0_f64;
        s = ((cosine - lead) / first_squared).clamp(0.0_f64, 1.0_f64);
    }
    length(sub(along(start, d1, s), along(from, d2, t)))
}
