//! \[DESIGN\] Exact tests on single rings, for orientation, point location
//! and nesting of rings that may touch at vertices. Each is decided by
//! exact orientation; only areas are floating, summed with compensation.

use std::cmp::Ordering;

use super::Point;
use super::label::Location;
use super::noding::deduplicated;
use super::predicates::orient2d;
use super::segment::{lexicographic, on_segment, same};

/// The shoelace sum is twice the area.
const HALF: f64 = 0.5;

/// A ring's vertices once each, with repeats removed and the closing
/// repeat of the first dropped.
pub(crate) fn open(ring: &[Point]) -> Vec<Point> {
    let mut vertices = deduplicated(ring);
    if vertices.len() > 1 && vertices.first().zip(vertices.last()).is_some_and(|(first, last)| same(*first, *last)) {
        vertices.pop();
    }
    vertices
}

/// The ring's turning direction, greater for counterclockwise, less for
/// clockwise, equal when it encloses no area. Decided at its least
/// vertex, a corner of its convex hull, where its two neighbours turn the
/// ring's way unless they are collinear with it; then the sign of the
/// compensated area decides.
pub(crate) fn orientation(ring: &[Point]) -> Ordering {
    let vertices = open(ring);
    let count = vertices.len();
    let Some((least, _)) = vertices.iter().enumerate().min_by(|(_, left), (_, right)| lexicographic(**left, **right)) else {
        return Ordering::Equal;
    };
    let before = vertices.get(least.checked_sub(1).unwrap_or(count.saturating_sub(1))).copied();
    let after = vertices.get(least.saturating_add(1).checked_rem(count).unwrap_or_default()).copied();
    let turn = match (before, vertices.get(least).copied(), after) {
        (Some(before), Some(at), Some(after)) => orient2d(before, at, after),
        _ => Ordering::Equal,
    };
    if turn != Ordering::Equal {
        return turn;
    }
    signed_area(ring).partial_cmp(&0.0_f64).unwrap_or(Ordering::Equal)
}

/// The signed area by the shoelace formula, with Neumaier's
/// compensated summation; positive for counterclockwise rings.
pub(crate) fn signed_area(ring: &[Point]) -> f64 {
    let Some(origin) = ring.first().copied() else { return 0.0_f64 };
    let [ox, oy] = origin;
    let mut sum = Compensated::default();
    for pair in ring.windows(2) {
        if let [[ax, ay], [bx, by]] = pair {
            sum.add((ax - ox) * (by - oy));
            sum.add(-(bx - ox) * (ay - oy));
        }
    }
    sum.total() * HALF
}

/// Neumaier's variant of Kahan summation.
#[derive(Default, Clone, Copy)]
pub(crate) struct Compensated {
    sum: f64,
    correction: f64,
}

impl Compensated {
    pub(crate) fn add(&mut self, value: f64) {
        let total = self.sum + value;
        if self.sum.abs() >= value.abs() {
            self.correction += (self.sum - total) + value;
        } else {
            self.correction += (value - total) + self.sum;
        }
        self.sum = total;
    }

    pub(crate) fn total(self) -> f64 {
        self.sum + self.correction
    }
}

/// Where `point` lies relative to the closed region a ring bounds, by a
/// half-open ray crossing count, each crossing decided exactly.
pub(crate) fn locate(point: Point, ring: &[Point]) -> Location {
    let [_, y] = point;
    let mut inside = false;
    for pair in ring.windows(2) {
        let [a, b] = [pair.first(), pair.last()].map(|vertex| vertex.copied().unwrap_or_default());
        if on_segment(point, a, b) {
            return Location::Boundary;
        }
        let ([_, ay], [_, by]) = (a, b);
        if (ay > y) != (by > y) {
            let (bottom, top) = if by > ay { (a, b) } else { (b, a) };
            if orient2d(bottom, top, point) == Ordering::Greater {
                inside = !inside;
            }
        }
    }
    if inside { Location::Interior } else { Location::Exterior }
}

/// Whether the region of `inner` lies within the region of `outer`, for
/// rings that do not cross and share at most vertices. A vertex of
/// `inner` off `outer` decides; if every vertex is on it, an edge of
/// `inner` leaving a shared vertex decides by the sector it enters.
pub(crate) fn contains(outer: &[Point], inner: &[Point]) -> bool {
    for vertex in inner {
        match locate(*vertex, outer) {
            Location::Interior => return true,
            Location::Exterior => return false,
            Location::Boundary => (),
        }
    }
    let inner_open = open(inner);
    let outer_open = open(outer);
    for (index, vertex) in inner_open.iter().enumerate() {
        let next = inner_open.get(index.saturating_add(1).checked_rem(inner_open.len()).unwrap_or_default()).copied().unwrap_or(*vertex);
        if let Some(inside) = enters(&outer_open, *vertex, next) {
            return inside;
        }
    }
    false
}

/// Whether the direction from `at`, a vertex of the open ring `ring`,
/// toward `toward` enters the ring's region, or nothing if it runs along
/// one of the ring's edges there or `at` is no vertex of the ring.
fn enters(ring: &[Point], at: Point, toward: Point) -> Option<bool> {
    let count = ring.len();
    let position = ring.iter().position(|vertex| same(*vertex, at))?;
    let before = ring.get(position.checked_sub(1).unwrap_or(count.saturating_sub(1))).copied()?;
    let after = ring.get(position.saturating_add(1).checked_rem(count).unwrap_or_default()).copied()?;
    if same(toward, before) || same(toward, after) {
        return None;
    }
    let convex = orient2d(before, at, after) == Ordering::Greater;
    let left_of_out = orient2d(at, after, toward) == Ordering::Greater;
    let left_of_in = orient2d(before, at, toward) == Ordering::Greater;
    let left = if convex { left_of_out && left_of_in } else { left_of_out || left_of_in };
    let counterclockwise = orientation(ring) == Ordering::Greater;
    Some(left == counterclockwise)
}
