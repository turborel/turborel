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
//! closed cell, so the edges whose envelopes meet it decide the point. In
//! a cell several rings touch, each ring's parity is followed apart and
//! the polygons combined from the cell's record (`several.rs`).

use std::cmp::Ordering;

use super::super::tree::EdgeTree;
use super::several::{MOST_RINGS, NO_HOLE, SHELL_HOLDS};
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
    tree.within(cell, |a, b, _| {
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

/// The rings a record keeps, each one's parity and whether an edge of it
/// holds the point, one bit each.
struct Slots {
    rings: [u32; MOST_RINGS],
    count: usize,
    odd: u64,
    on: u64,
}

impl Slots {
    /// The rings of `record`, with their parities at the reference point.
    fn read(record: &[u32]) -> Option<Self> {
        let mut slots = Self { rings: [0; MOST_RINGS], count: 0, odd: 0, on: 0 };
        let mut words = record.iter().copied();
        for _ in 0..words.next()? {
            let [_, _, count] = [words.next()?, words.next()?, words.next()?];
            for _ in 0..count {
                let word = words.next()?;
                *slots.rings.get_mut(slots.count)? = word >> 1_u32;
                slots.odd |= u64::from(word & 1) << slots.count;
                slots.count = slots.count.checked_add(1)?;
            }
        }
        Some(slots)
    }

    /// The bit of ring `ring`, none if the record does not keep it.
    fn bit(&self, ring: usize) -> Option<u64> {
        let ring = u32::try_from(ring).ok()?;
        let at = self.rings.get(..self.count)?.iter().position(|&kept| kept == ring)?;
        1_u64.checked_shl(u32::try_from(at).ok()?)
    }

    /// Where the point lies against the ring of slot `slot`.
    fn location(&self, slot: usize) -> Location {
        let bit = 1_u64.checked_shl(u32::try_from(slot).unwrap_or(u32::MAX)).unwrap_or(0);
        if self.on & bit != 0 {
            Location::Boundary
        } else if self.odd & bit != 0 {
            Location::Interior
        } else {
            Location::Exterior
        }
    }
}

/// Where `point`, in the cell at `row` and `col`, lies in the polygon,
/// from the cell's record (`several.rs`): each kept ring's parity at the
/// reference point, flipped by its edges that cross the point's path.
/// Rings the record leaves out belong to polygons that hold none of the
/// cell, so their edges change nothing.
pub(super) fn locate_several(grid: &Grid, tree: &EdgeTree, point: Point, (row, col): (usize, usize), record: &[u32]) -> Option<Location> {
    let mut slots = Slots::read(record)?;
    let right = grid.column_start(col.checked_add(1)?)?;
    let cell = [grid.column_start(col)?, grid.row_start(row)?, right, grid.row_start(row.checked_add(1)?)?];
    let path = Path { point, right, centre: grid.centre(row)? };
    tree.within(cell, |a, b, ring| {
        let Some(bit) = slots.bit(ring) else { return true };
        match path.event(a, b) {
            Event::On => slots.on |= bit,
            Event::Crosses(true) => slots.odd ^= bit,
            Event::Crosses(false) => (),
        }
        true
    })?;
    combine(record, &slots)
}

/// The polygons of a record combined as exact location combines them: a
/// shell that does not hold the point decides its polygon; otherwise the
/// first hole, in order, that holds the point or has it on its boundary
/// does; and a point inside any polygon is inside.
fn combine(record: &[u32], slots: &Slots) -> Option<Location> {
    let mut words = record.iter().copied();
    let (mut slot, mut boundary) = (0_usize, false);
    for _ in 0..words.next()? {
        let [shell, holding, count] = [words.next()?, words.next()?, words.next()?];
        let mut location = Location::Interior;
        let mut decided = false;
        for at in 0..count {
            let ring = words.next()? >> 1_u32;
            let found = slots.location(slot);
            slot = slot.checked_add(1)?;
            let hole = at > 0 || shell == SHELL_HOLDS;
            let passed = hole && (ring > holding || found == Location::Exterior);
            if decided || passed {
                continue;
            }
            location = if hole && found == Location::Interior { Location::Exterior } else { found };
            decided = location != Location::Interior;
        }
        if !decided && holding != NO_HOLE {
            location = Location::Exterior;
        }
        match location {
            Location::Interior => return Some(Location::Interior),
            Location::Boundary => boundary = true,
            Location::Exterior => (),
        }
    }
    Some(if boundary { Location::Boundary } else { Location::Exterior })
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
