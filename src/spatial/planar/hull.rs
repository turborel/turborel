//! ST_ConvexHull [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.29], by
//! Andrew's monotone chain over every coordinate, each turn decided by
//! exact orientation. Collinear points are dropped, so a hull of one
//! position is a point, of collinear positions a line between the two
//! extremes, and otherwise a counterclockwise polygon.

use std::cmp::Ordering;

use super::Point;
use super::predicates::orient2d;
use super::segment::{lexicographic, same};
use super::shape::Geometry;

/// A hull polygon has at least three corners.
const CORNERS: usize = 3;

/// The convex hull, or the empty set for an empty geometry, for which
/// the method returns the null value [§5.1.29 Description 2a].
pub fn convex_hull(geometry: &Geometry) -> Geometry {
    let mut points = geometry.coordinates();
    points.sort_by(|left, right| lexicographic(*left, *right));
    points.dedup_by(|left, right| same(*left, *right));
    match points.as_slice() {
        [] => return Geometry::Empty,
        [only] => return Geometry::Point(*only),
        _ => (),
    }
    let lower = chain(points.iter());
    let upper = chain(points.iter().rev());
    let mut ring: Vec<Point> = lower.iter().take(lower.len().saturating_sub(1)).copied().collect();
    ring.extend(upper.iter().take(upper.len().saturating_sub(1)));
    if ring.len() < CORNERS {
        let (Some(first), Some(last)) = (points.first(), points.last()) else { return Geometry::Empty };
        return Geometry::Line(vec![*first, *last]);
    }
    ring.extend(ring.first().copied());
    Geometry::Polygon(vec![ring])
}

/// One half of the hull. Each point pops the points it makes a
/// clockwise or straight turn with.
fn chain<'a>(points: impl Iterator<Item = &'a Point>) -> Vec<Point> {
    let mut hull: Vec<Point> = Vec::new();
    for point in points {
        while let [.., before, last] = hull.as_slice() {
            if orient2d(*before, *last, *point) == Ordering::Greater {
                break;
            }
            hull.pop();
        }
        hull.push(*point);
    }
    hull
}
