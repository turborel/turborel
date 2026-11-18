//! Minimum distance between disjoint XY point sets. Intersection and
//! emptiness are checked by the caller; the remaining minimum is attained
//! by points or line segments on their boundaries. Empty components
//! contribute infinity, never a spurious zero or an early overflow error.

mod near;

use super::crossing::segments;
use super::robust::distance::{point, segment};
use super::shape::{Point, Shape};
use crate::error::Result;
use crate::spatial::{Bounds, DIMENSIONS};

/// A deliberately conservative domain for an error-free distance scan.
/// Differences are at most MAX/4 and point L1 norms at most MAX/2.
/// A perpendicular numerator divided by the greatest direction component
/// is at most twice the greatest offset, also MAX/2. The bounded norms
/// and exact scale factors leave ample room for magnitude rounding.
/// Larger coordinates remain valid but require ordinary evaluation (D15).
const FINITE_COORDINATE: f64 = f64::MAX / 8.0;

pub(crate) fn distance_domain() -> Result<Bounds> {
    Bounds::new([-FINITE_COORDINATE; DIMENSIONS], [FINITE_COORDINATE; DIMENSIONS])
}

pub(super) fn separated(left: &Shape, right: &Shape) -> f64 {
    match (left, right) {
        (Shape::Multi(_, parts), _) => minimum(parts.iter().map(|part| separated(part, right))),
        (_, Shape::Multi(_, parts)) => minimum(parts.iter().map(|part| separated(left, part))),
        (Shape::Point(p), _) => p.map_or(f64::INFINITY, |p| to_point(p, right)),
        (_, Shape::Point(p)) => p.map_or(f64::INFINITY, |p| to_point(p, left)),
        (Shape::Line(left), Shape::Line(right)) => lines(left, right),
        (Shape::Line(line), Shape::Polygon(rings)) | (Shape::Polygon(rings), Shape::Line(line)) => {
            minimum(rings.iter().map(|ring| lines(line, ring)))
        }
        (Shape::Polygon(left), Shape::Polygon(right)) => minimum(left.iter().flat_map(|ring| right.iter().map(|other| lines(ring, other)))),
    }
}

fn to_point(p: Point, shape: &Shape) -> f64 {
    match shape {
        Shape::Point(other) => other.map_or(f64::INFINITY, |other| point(p, other)),
        Shape::Line(line) => minimum(segments(line).map(|(a, b)| segment(p, a, b))),
        Shape::Polygon(rings) => minimum(rings.iter().flat_map(|ring| segments(ring).map(|(a, b)| segment(p, a, b)))),
        Shape::Multi(_, parts) => minimum(parts.iter().map(|part| to_point(p, part))),
    }
}

/// The least measure between the chains' edges: by an expanding search
/// once the pairs are many enough to repay it, else, or where its
/// arithmetic declines, over every pair.
fn lines(left: &[Point], right: &[Point]) -> f64 {
    let pairs = left.len().saturating_sub(1).saturating_mul(right.len().saturating_sub(1));
    let searched = (pairs >= SEARCH_PAIRS).then(|| near::nearest(left, right)).flatten();
    searched.unwrap_or_else(|| minimum(segments(left).flat_map(|(a, b)| segments(right).map(move |(c, d)| between(a, b, c, d)))))
}

/// \[DESIGN\] Edge pairs from which an expanding search repays building
/// its hierarchy.
const SEARCH_PAIRS: usize = 4_096;

fn between(a: Point, b: Point, c: Point, d: Point) -> f64 {
    segment(a, c, d).min(segment(b, c, d)).min(segment(c, a, b)).min(segment(d, a, b))
}

fn minimum(values: impl Iterator<Item = f64>) -> f64 {
    values.fold(f64::INFINITY, f64::min)
}
