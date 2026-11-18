//! Planar measures, ST_Area [ISO/IEC 13249-3:2016 (DIS, SC32 N2593)
//! §8.1.2, §10.5.3], ST_Length [§7.1.2, §10.3.5], ST_Perimeter [§8.1.4,
//! §10.5.5], ST_Centroid [§8.1.6, §10.5.7] and ST_PointOnSurface [§8.1.8,
//! §10.5.9]. Area, length and centroid are implementation-defined; here
//! they are the Euclidean ones in coordinate units, summed with
//! compensation. Lengths avoid the platform's `hypot`, so results are the
//! same everywhere.

use std::cmp::Ordering;

use super::Point;
use super::label::Location;
use super::ring::{Compensated, locate, signed_area};
use super::segment::compare;
use super::shape::Geometry;

/// A third, for a triangle's centroid.
const THIRD: f64 = 1.0 / 3.0;
/// A half, for midpoints.
const HALF: f64 = 0.5;

/// The Euclidean length of a vector, scaled so squaring cannot overflow
/// or underflow, from correctly rounded operations only.
pub(crate) fn norm(x: f64, y: f64) -> f64 {
    let (large, small) = if x.abs() >= y.abs() { (x.abs(), y.abs()) } else { (y.abs(), x.abs()) };
    if large <= 0.0_f64 || !large.is_finite() {
        return large;
    }
    let ratio = small / large;
    large * (1.0_f64 + ratio * ratio).sqrt()
}

fn line_length(line: &[Point]) -> f64 {
    let mut sum = Compensated::default();
    for pair in line.windows(2) {
        if let [[ax, ay], [bx, by]] = pair {
            sum.add(norm(bx - ax, by - ay));
        }
    }
    sum.total()
}

/// The area of the polygons, each exterior ring's less its interior
/// rings', or the null value for an empty geometry.
pub fn area(geometry: &Geometry) -> Option<f64> {
    let parts = geometry.parts();
    if parts.is_empty() {
        return None;
    }
    let mut sum = Compensated::default();
    for rings in &parts.polygons {
        for (index, ring) in rings.iter().enumerate() {
            let magnitude = signed_area(ring).abs();
            sum.add(if index == 0 { magnitude } else { -magnitude });
        }
    }
    Some(sum.total())
}

/// The length of the curves, or the null value for an empty geometry.
pub fn length(geometry: &Geometry) -> Option<f64> {
    let parts = geometry.parts();
    if parts.is_empty() {
        return None;
    }
    let mut sum = Compensated::default();
    parts.lines.iter().for_each(|line| sum.add(line_length(line)));
    Some(sum.total())
}

/// The length of the surfaces' boundaries, the length of ST_Boundary
/// [§8.1.4 Definition], or the null value for an empty geometry.
pub fn perimeter(geometry: &Geometry) -> Option<f64> {
    let parts = geometry.parts();
    if parts.is_empty() {
        return None;
    }
    let mut sum = Compensated::default();
    parts.polygons.iter().flatten().for_each(|ring| sum.add(line_length(ring)));
    Some(sum.total())
}

/// The mathematical centroid of the surfaces [§8.1.6, §10.5.7], or the
/// null value if there are none.
pub fn centroid(geometry: &Geometry) -> Option<Point> {
    surface_centroid(&geometry.parts().polygons)
}

/// Triangle fans from each ring's first vertex, taking moments of signed
/// triangle areas about that vertex, so interior rings subtract.
fn surface_centroid(polygons: &[Vec<Vec<Point>>]) -> Option<Point> {
    let (mut total, mut x, mut y) = (Compensated::default(), Compensated::default(), Compensated::default());
    for rings in polygons {
        for (index, ring) in rings.iter().enumerate() {
            let sign = if (signed_area(ring) >= 0.0_f64) == (index == 0) { 1.0_f64 } else { -1.0_f64 };
            let Some([ox, oy]) = ring.first().copied() else { continue };
            for pair in ring.windows(2) {
                let [[ax, ay], [bx, by]] = [pair.first(), pair.last()].map(|point| point.copied().unwrap_or_default());
                let area = sign * HALF * ((ax - ox) * (by - oy) - (bx - ox) * (ay - oy));
                total.add(area);
                x.add(area * (ox + ax + bx) * THIRD);
                y.add(area * (oy + ay + by) * THIRD);
            }
        }
    }
    let total = total.total();
    (total.abs() > 0.0_f64).then(|| [x.total() / total, y.total() / total])
}

/// A point of the geometry, inside a surface where there is one. On a
/// horizontal line between two vertex heights it is the middle of the
/// widest stretch inside, checked exactly. If no such middle is inside,
/// it is a vertex, which lies on the closed set. The null value for an empty
/// geometry.
pub fn point_on_surface(geometry: &Geometry) -> Option<Point> {
    let parts = geometry.parts();
    let mut polygons: Vec<&Vec<Vec<Point>>> = parts.polygons.iter().collect();
    polygons.sort_by(|left, right| polygon_area(right.as_slice()).total_cmp(&polygon_area(left.as_slice())));
    for rings in polygons {
        if let Some(point) = inside(rings) {
            return Some(point);
        }
    }
    let line = parts.lines.iter().find(|line| !line.is_empty());
    let middle = line.and_then(|line| line.get(line.len().checked_div(2).unwrap_or_default()));
    middle
        .or_else(|| parts.points.first())
        .or_else(|| parts.polygons.first().and_then(|rings| rings.first()).and_then(|ring| ring.first()))
        .copied()
}

fn polygon_area(rings: &[Vec<Point>]) -> f64 {
    rings.iter().enumerate().map(|(index, ring)| if index == 0 { signed_area(ring).abs() } else { -signed_area(ring).abs() }).sum()
}

fn located(point: Point, rings: &[Vec<Point>]) -> Location {
    let Some((shell, holes)) = rings.split_first() else { return Location::Exterior };
    match locate(point, shell) {
        Location::Interior => (),
        other @ (Location::Boundary | Location::Exterior) => return other,
    }
    for hole in holes {
        match locate(point, hole) {
            Location::Interior => return Location::Exterior,
            Location::Boundary => return Location::Boundary,
            Location::Exterior => (),
        }
    }
    Location::Interior
}

fn inside(rings: &[Vec<Point>]) -> Option<Point> {
    let mut heights: Vec<f64> = rings.iter().flatten().map(|[_, y]| *y).collect();
    heights.sort_by(|left, right| compare(*left, *right));
    heights.dedup_by(|left, right| compare(*left, *right) == Ordering::Equal);
    let (Some(low), Some(high)) = (heights.first().copied(), heights.last().copied()) else { return None };
    let centre = low * HALF + high * HALF;
    let mut lines: Vec<f64> = heights.windows(2).map(|pair| pair.iter().sum::<f64>() * HALF).collect();
    lines.sort_by(|left, right| (left - centre).abs().total_cmp(&(right - centre).abs()));
    lines.into_iter().find_map(|y| widest(rings, y).filter(|point| located(*point, rings) == Location::Interior))
}

/// The middle of the widest stretch of the line at height `y` between
/// consecutive ring crossings.
fn widest(rings: &[Vec<Point>], y: f64) -> Option<Point> {
    let mut crossings: Vec<f64> = Vec::new();
    for pair in rings.iter().flat_map(|ring| ring.windows(2)) {
        let [[ax, ay], [bx, by]] = [pair.first(), pair.last()].map(|point| point.copied().unwrap_or_default());
        if (ay > y) != (by > y) {
            crossings.push(ax + (y - ay) * (bx - ax) / (by - ay));
        }
    }
    crossings.sort_by(|left, right| compare(*left, *right));
    crossings
        .chunks(2)
        .filter_map(|pair| if let [left, right] = pair { Some((*left, *right)) } else { None })
        .max_by(|(a, b), (c, d)| (b - a).total_cmp(&(d - c)))
        .map(|(left, right)| [left * HALF + right * HALF, y])
}
