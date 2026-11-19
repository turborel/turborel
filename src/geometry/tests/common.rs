//! Shared geometry fixtures for kernel and prepared-function unit tests.

use crate::geometry::crossing::segments;
use crate::geometry::shape::Point;

/// A square with many collinear edges, retaining exact axis-aligned
/// boundary points while exercising a prepared interval tree.
pub(in crate::geometry) fn dense_ring(low: f64, high: f64) -> Vec<Point> {
    let corners = [[low, low], [high, low], [high, high], [low, high], [low, low]];
    subdivided_ring(&corners)
}

/// Adds collinear points without changing the supplied closed boundary.
pub(in crate::geometry) fn subdivided_ring(corners: &[Point]) -> Vec<Point> {
    let mut points = Vec::new();
    for (a, b) in segments(corners) {
        for step in 0..16_u32 {
            let t = f64::from(step) / 16.0_f64;
            let [ax, ay] = a;
            let [bx, by] = b;
            points.push([ax * (1.0_f64 - t) + bx * t, ay * (1.0_f64 - t) + by * t]);
        }
    }
    points.push(corners[0]);
    points
}
