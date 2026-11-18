//! Locating a point in a boundary cell that only one ring's edges may
//! touch (RULES.md D85). The sweep kept the ring's crossing parity at the
//! cell's reference point, where its right side meets its row's centre
//! line, from the crossings of that line right of it. The horizontal ray
//! exact location casts from the point is replaced by a path to that
//! reference: a run right along the point's height to the cell's right
//! side, then a rise along that side to the centre line. Under the
//! half-open rule the run and the centre line lie just above the heights
//! they name; the rise lies just left of the side, nearer to it than they
//! lie above their heights. No vertex lies on the path, so the ray and the
//! path differ by a closed loop through no vertex, and a ring crosses them
//! equally often, modulo two. An edge through a corner of the path is
//! placed by its slope, as the sweep places a crossing on a cell's side,
//! so every point is decided. Every edge the path can cross touches the
//! closed cell, so the edges whose envelopes meet it decide the point.

use std::cmp::Ordering;

use super::super::tree::EdgeTree;
use crate::geometry::crossing::{Location, on_segment};
use crate::geometry::grid::Grid;
use crate::geometry::robust::orient;
use crate::geometry::shape::Point;

/// The path from a point to its cell's reference point: right along the
/// point's height to `right`, the cell's right side, then along it to the
/// row's centre line at `centre`.
struct Path {
    point: Point,
    right: f64,
    centre: f64,
}

/// What an edge does to the point: holds it, or crosses its path an odd
/// number of times, or an even one.
enum Event {
    On,
    Crosses(bool),
}

/// Where `point`, in the cell at `row` and `col`, lies in the polygon,
/// given whether the cell's reference point is `inside` it.
pub(super) fn locate(grid: &Grid, tree: &EdgeTree, point: Point, (row, col): (usize, usize), inside: bool) -> Option<Location> {
    let right = grid.column_start(col.checked_add(1)?)?;
    let cell = [grid.column_start(col)?, grid.row_start(row)?, right, grid.row_start(row.checked_add(1)?)?];
    let path = Path { point, right, centre: grid.centre(row)? };
    // The parity so far, or none once an edge holds the point.
    let mut odd = Some(inside);
    tree.within(cell, |a, b| {
        let Some(parity) = odd else { return false };
        odd = match path.event(a, b) {
            Event::On => None,
            Event::Crosses(crosses) => Some(parity != crosses),
        };
        odd.is_some()
    })?;
    Some(match odd {
        Some(true) => Location::Interior,
        Some(false) => Location::Exterior,
        None => Location::Boundary,
    })
}

impl Path {
    /// What edge `a`-`b` does to the point.
    fn event(&self, a: Point, b: Point) -> Event {
        if on_segment(self.point, a, b) {
            return Event::On;
        }
        let [_, y] = self.point;
        let run = straddles(a, b, y) && across(a, b, self.point) == Ordering::Greater && !beyond(a, b, [self.right, y]);
        let level = (y - self.centre).abs() <= 0.0_f64;
        Event::Crosses(run != (!level && self.rise(a, b)))
    }

    /// Whether the edge crosses the rise, just left of the right side,
    /// from just above the point's height to just above the centre line:
    /// an end on the side lies right of the rise, and a crossing at either
    /// height below the rise's end there.
    fn rise(&self, a: Point, b: Point) -> bool {
        let ([ax, _], [bx, _], [_, y]) = (a, b, self.point);
        let x = self.right;
        if (ax < x) == (bx < x) {
            return false;
        }
        let above = |h: f64| height(a, b, [x, h]) == Ordering::Greater;
        above(y) != above(self.centre)
    }
}

/// Whether an edge crosses height `y` by the half-open rule: one end above
/// it and the other not.
pub(super) fn straddles([_, ay]: Point, [_, by]: Point, y: f64) -> bool {
    (ay > y) != (by > y)
}

/// Whether an edge that straddles `q`'s height crosses it, just above that
/// height, right of a line just left of `q`: right of `q`, or through `q`
/// unless it leans left as it rises.
pub(super) fn beyond(a: Point, b: Point, q: Point) -> bool {
    match across(a, b, q) {
        Ordering::Greater => true,
        Ordering::Less => false,
        Ordering::Equal => !leans_left(a, b),
    }
}

/// Whether an edge's x falls as its y rises.
fn leans_left([ax, ay]: Point, [bx, by]: Point) -> bool {
    if by > ay { bx < ax } else { bx > ax }
}

/// Where an edge that straddles `q`'s height crosses it, against `q`:
/// right of it, through it, or left of it. Its x-extent decides when it
/// can; exact orientation otherwise.
fn across(a: Point, b: Point, q: Point) -> Ordering {
    let ([ax, ay], [bx, by], [qx, _]) = (a, b, q);
    if ax.min(bx) > qx {
        return Ordering::Greater;
    }
    if ax.max(bx) < qx {
        return Ordering::Less;
    }
    let turn = orient(a, b, q);
    if by > ay { turn } else { turn.reverse() }
}

/// Where an edge with ends on either side of `q`'s x, or one end on it,
/// meets that line, against `q`: above it, through it, or below it. Its
/// y-extent decides when it can; exact orientation otherwise.
fn height(a: Point, b: Point, q: Point) -> Ordering {
    let ([ax, ay], [bx, by], [_, qy]) = (a, b, q);
    if ay.min(by) > qy {
        return Ordering::Greater;
    }
    if ay.max(by) < qy {
        return Ordering::Less;
    }
    let turn = orient(a, b, q);
    if bx > ax { turn.reverse() } else { turn }
}
