//! The exact point-in-ring rule of the prepared point locator: a ray
//! from the point crosses a ring's edges an odd number of times when the
//! point is inside, and a point on an edge is on the boundary. Every
//! other relationship comes from the planar engine (`spatial::planar`).

use std::cmp::Ordering;

use super::robust::orient;
use super::shape::Point;

/// The two endpoints of a line segment.
const ENDPOINTS: usize = 2;

/// Where a point lies relative to a polygon.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Location {
    Exterior,
    Boundary,
    Interior,
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
