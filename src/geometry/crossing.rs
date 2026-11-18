//! The exact point-in-ring rule of the prepared point locator: a ray
//! from the point crosses a ring's edges an odd number of times when the
//! point is inside, and a point on an edge is on the boundary; and a
//! point's location in a shape composed of rings, exactly. Every other
//! relationship comes from the planar engine (`spatial::planar`).

use std::cmp::Ordering;

use super::robust::orient;
use super::shape::Point;
#[cfg(test)]
use super::shape::Shape;

/// The two endpoints of a line segment.
const ENDPOINTS: usize = 2;

/// Where a point lies relative to a polygon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Location {
    Exterior,
    Boundary,
    Interior,
}

/// Where `point` lies in the closed point set of `shape`, exactly: a
/// polygon's interior is its shell's less its holes', each ring's by the
/// ray rule; a line's boundary is its two ends; a collection's parts
/// combine as point sets.
#[cfg(test)]
pub(super) fn locate(point: Point, shape: &Shape) -> Location {
    match shape {
        Shape::Point(other) => {
            if other.is_some_and(|other| same(point, other)) {
                Location::Interior
            } else {
                Location::Exterior
            }
        }
        Shape::Line(line) => on_line(point, line),
        Shape::Polygon(rings) => in_polygon(point, rings),
        Shape::Multi(_, parts) => {
            let mut location = Location::Exterior;
            for part in parts {
                match locate(point, part) {
                    Location::Interior => return Location::Interior,
                    Location::Boundary => location = Location::Boundary,
                    Location::Exterior => (),
                }
            }
            location
        }
    }
}

#[cfg(test)]
fn same(left: Point, right: Point) -> bool {
    left.map(f64::to_bits) == right.map(f64::to_bits)
}

#[cfg(test)]
fn on_line(point: Point, line: &[Point]) -> Location {
    if !segments(line).any(|(start, end)| on_segment(point, start, end)) {
        return Location::Exterior;
    }
    let (Some(first), Some(last)) = (line.first(), line.last()) else { return Location::Exterior };
    if !same(*first, *last) && (same(point, *first) || same(point, *last)) { Location::Boundary } else { Location::Interior }
}

/// Where `point` lies in the polygon of `rings`, its shell first.
#[cfg(test)]
pub(super) fn in_polygon(point: Point, rings: &[Vec<Point>]) -> Location {
    let Some((shell, holes)) = rings.split_first() else { return Location::Exterior };
    let shell = in_ring(point, shell);
    if shell != Location::Interior {
        return shell;
    }
    for hole in holes {
        match in_ring(point, hole) {
            Location::Interior => return Location::Exterior,
            Location::Boundary => return Location::Boundary,
            Location::Exterior => (),
        }
    }
    Location::Interior
}

/// A half-open ray crossing avoids counting a ring vertex twice. Exact
/// orientation decides on which side the crossing lies without division.
#[cfg(test)]
fn in_ring(point: Point, ring: &[Point]) -> Location {
    let mut ray = Ray::new(point);
    for (a, b) in segments(ring) {
        if ray.add(a, b) {
            return Location::Boundary;
        }
    }
    ray.location()
}

fn in_bounds([x, y]: Point, [ax, ay]: Point, [bx, by]: Point) -> bool {
    x >= ax.min(bx) && x <= ax.max(bx) && y >= ay.min(by) && y <= ay.max(by)
}

pub(super) fn on_segment(point: Point, start: Point, end: Point) -> bool {
    in_bounds(point, start, end) && orient(start, end, point) == Ordering::Equal
}

pub(super) fn segments(line: &[Point]) -> impl Iterator<Item = (Point, Point)> + '_ {
    line.windows(ENDPOINTS).filter_map(|pair| if let [start, end] = pair { Some((*start, *end)) } else { None })
}

/// Both sequential and indexed ring visits use this same exact crossing
/// rule. Edge order does not change parity or boundary membership.
pub(super) struct Ray {
    point: Point,
    inside: bool,
}

impl Ray {
    pub(super) fn new(point: Point) -> Self {
        Self { point, inside: false }
    }

    pub(super) fn point(&self) -> Point {
        self.point
    }

    pub(super) fn add(&mut self, a: Point, b: Point) -> bool {
        if on_segment(self.point, a, b) {
            return true;
        }
        let [_, y] = self.point;
        let [_, ay] = a;
        let [_, by] = b;
        if (ay > y) != (by > y) {
            let turn = orient(a, b, self.point);
            if (turn == Ordering::Greater) == (by > ay) {
                self.inside = !self.inside;
            }
        }
        false
    }

    pub(super) fn location(&self) -> Location {
        if self.inside { Location::Interior } else { Location::Exterior }
    }
}
