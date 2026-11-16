//! Closed-set intersection for finite XY simple features (OGC 06-103r4
//! §6.1.2.3). Polygon interiors use the exterior ring minus hole
//! interiors; their boundaries remain included. As in that model,
//! polygon rings must describe valid topology. Structural decoding alone
//! does not establish polygon validity.

use std::cmp::Ordering;

use super::robust::orient;
use super::shape::{Point, Shape};

/// The two endpoints of a line segment.
const ENDPOINTS: usize = 2;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Location {
    Exterior,
    Boundary,
    Interior,
}

pub(super) fn intersects(left: &Shape, right: &Shape) -> bool {
    match (left, right) {
        (Shape::Multi(_, parts), _) => parts.iter().any(|part| intersects(part, right)),
        (_, Shape::Multi(_, parts)) => parts.iter().any(|part| intersects(left, part)),
        (Shape::Point(point), _) => point.is_some_and(|point| locate(point, right) != Location::Exterior),
        (_, Shape::Point(point)) => point.is_some_and(|point| locate(point, left) != Location::Exterior),
        (Shape::Line(left), Shape::Line(right)) => lines(left, right),
        (Shape::Line(line), Shape::Polygon(rings)) | (Shape::Polygon(rings), Shape::Line(line)) => line_polygon(line, rings),
        (Shape::Polygon(left), Shape::Polygon(right)) => polygons(left, right),
    }
}

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

fn same(left: Point, right: Point) -> bool {
    left.map(f64::to_bits) == right.map(f64::to_bits)
}

fn in_bounds([x, y]: Point, [ax, ay]: Point, [bx, by]: Point) -> bool {
    x >= ax.min(bx) && x <= ax.max(bx) && y >= ay.min(by) && y <= ay.max(by)
}

pub(super) fn on_segment(point: Point, start: Point, end: Point) -> bool {
    in_bounds(point, start, end) && orient(start, end, point) == Ordering::Equal
}

fn on_line(point: Point, line: &[Point]) -> Location {
    if !segments(line).any(|(start, end)| on_segment(point, start, end)) {
        return Location::Exterior;
    }
    let (Some(first), Some(last)) = (line.first(), line.last()) else { return Location::Exterior };
    if !same(*first, *last) && (same(point, *first) || same(point, *last)) { Location::Boundary } else { Location::Interior }
}

pub(super) fn segments(line: &[Point]) -> impl Iterator<Item = (Point, Point)> + '_ {
    line.windows(ENDPOINTS).filter_map(|pair| if let [start, end] = pair { Some((*start, *end)) } else { None })
}

pub(super) fn segments_intersect(a: Point, b: Point, c: Point, d: Point) -> bool {
    if !overlap(a, b, c, d) {
        return false;
    }
    let first = orient(a, b, c);
    let second = orient(a, b, d);
    let third = orient(c, d, a);
    let fourth = orient(c, d, b);
    let touches = |order, point, start, end| order == Ordering::Equal && in_bounds(point, start, end);
    if touches(first, c, a, b) || touches(second, d, a, b) || touches(third, a, c, d) || touches(fourth, b, c, d) {
        return true;
    }
    opposite(first, second) && opposite(third, fourth)
}

fn overlap([ax, ay]: Point, [bx, by]: Point, [cx, cy]: Point, [dx, dy]: Point) -> bool {
    ax.min(bx) <= cx.max(dx) && cx.min(dx) <= ax.max(bx) && ay.min(by) <= cy.max(dy) && cy.min(dy) <= ay.max(by)
}

fn opposite(left: Ordering, right: Ordering) -> bool {
    matches!((left, right), (Ordering::Less, Ordering::Greater) | (Ordering::Greater, Ordering::Less))
}

fn lines(left: &[Point], right: &[Point]) -> bool {
    segments(left).any(|(a, b)| segments(right).any(|(c, d)| segments_intersect(a, b, c, d)))
}

fn line_polygon(line: &[Point], rings: &[Vec<Point>]) -> bool {
    rings.iter().any(|ring| lines(line, ring)) || line.first().is_some_and(|point| in_polygon(*point, rings) != Location::Exterior)
}

fn polygons(left: &[Vec<Point>], right: &[Vec<Point>]) -> bool {
    if left.iter().any(|ring| right.iter().any(|other| lines(ring, other))) {
        return true;
    }
    let inside = |rings: &[Vec<Point>], other: &[Vec<Point>]| {
        rings.first().and_then(|ring| ring.first()).is_some_and(|point| in_polygon(*point, other) != Location::Exterior)
    };
    inside(left, right) || inside(right, left)
}

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
fn in_ring(point: Point, ring: &[Point]) -> Location {
    let mut ray = Ray::new(point);
    for (a, b) in segments(ring) {
        let boundary = ray.add(a, b);
        if boundary {
            return Location::Boundary;
        }
    }
    ray.location()
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
