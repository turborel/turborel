//! A prepared polygon's edges in a static hierarchy of their envelopes.
//! Each node splits its edges at the median along the axis whose halves'
//! envelopes overlap least, then whose halves are smaller, not on
//! alternate axes; edges with equal centres along it are ordered along the
//! other. So long parallel edges, a comb's teeth, fall into narrow leaves
//! even where they all share a centre's height. Point location
//! asks it for the edges meeting one grid cell (`cells.rs`); every edge it
//! returns is still tested exactly (D85). Point distance searches it for
//! the nearest edge, depth first and nearer child first, skipping only
//! subtrees and edges whose envelopes lie beyond the best measure so far,
//! widened as the chains' search widens it (`distance.rs`), so the minimum
//! is a scan's to the bit.

use std::cmp::Ordering;
use std::sync::Arc;

use super::shapes::rings;
use crate::geometry::crossing::segments;
use crate::geometry::distance::{reach, unit_of};
use crate::geometry::robust::distance::segment;
use crate::geometry::shape::{Point, Shape};
use crate::memory::{MemoryBudget, Reservation};

/// Edges per leaf: few exact tests per leaf, and few nodes per edge.
const LEAF: usize = 8;
/// A median split halves the edges, so no usize-sized count reaches this
/// depth.
const MAX_DEPTH: u32 = usize::BITS;
const HALF: f64 = 0.5;
/// Nodes a nearest search holds at once: below each level of the deepest
/// tree, MAX_DEPTH of them, the sibling not yet visited, with room to
/// spare.
const STACK: usize = 130;
/// 1 + 2^-40: a squared distance to an envelope, as computed, is within
/// 2^-50 of its own; one beyond the squared reach by this factor is beyond
/// the reach itself.
const SQUARED_SLACK: f64 = 1.000_000_000_000_909_5;

/// A closed envelope: lower x, lower y, upper x, upper y.
pub(super) type Window = [f64; 4];

struct Node {
    bounds: Window,
    /// The node after this one's subtree, in preorder.
    after: usize,
    /// A leaf's edges; an empty range for an inner node.
    start: usize,
    end: usize,
}

/// The edges of a polygon or multipolygon, in leaf order.
pub(super) struct EdgeTree {
    edges: Vec<(Point, Point)>,
    nodes: Vec<Node>,
    /// The largest coordinate magnitude of any edge.
    largest: f64,
    _held: Reservation,
}

impl EdgeTree {
    /// The hierarchy of `shape`'s edges, a polygon or multipolygon with
    /// `count` edges, if the budget grants the room.
    pub(super) fn new(shape: &Shape, count: usize, budget: &Arc<MemoryBudget>) -> Option<Box<Self>> {
        // A leaf holds at least half of LEAF edges, or all of fewer, so
        // there are at most count / (LEAF / 2) + 1 leaves and twice as
        // many nodes.
        let most = count.checked_div(LEAF.checked_div(2)?)?.checked_add(1)?.checked_mul(2)?;
        let bytes = count.checked_mul(size_of::<(Point, Point)>())?.checked_add(most.checked_mul(size_of::<Node>())?)?;
        let mut held = budget.reserve(bytes, "a prepared polygon's edge hierarchy").ok()?;
        let mut edges = Vec::new();
        edges.try_reserve_exact(count).ok()?;
        for ring in rings(shape)? {
            edges.extend(segments(ring));
        }
        let mut nodes = Vec::new();
        nodes.try_reserve_exact(most).ok()?;
        if !edges.is_empty() {
            build(&mut edges, 0, &mut nodes, 0)?;
        }
        let used = edges.capacity().checked_mul(size_of::<(Point, Point)>())?;
        held.resize(used.checked_add(nodes.capacity().checked_mul(size_of::<Node>())?)?).ok()?;
        let largest = edges.iter().flat_map(|&(a, b)| a.into_iter().chain(b)).map(f64::abs).fold(0.0_f64, f64::max);
        Some(Box::new(Self { edges, nodes, largest, _held: held }))
    }

    /// The least `segment(point, a, b)` over every edge, as a scan of them
    /// all takes it; none where the search's arithmetic cannot stay finite
    /// or the hierarchy is inconsistent. A subtree is skipped only when its
    /// envelope misses the square within reach of the best measure so far,
    /// so every edge whose measure could be least is measured.
    pub(super) fn nearest(&self, point: Point) -> Option<f64> {
        let [px, py] = point;
        let unit = unit_of(self.largest.max(px.abs()).max(py.abs()))?;
        let mut best = f64::INFINITY;
        let mut stack = [0_usize; STACK];
        let mut held = usize::from(!self.nodes.is_empty());
        while let Some(top) = held.checked_sub(1) {
            held = top;
            let at = *stack.get(top)?;
            let node = self.nodes.get(at)?;
            if beyond(point, node.bounds, reach(best, unit)) {
                continue;
            }
            if node.end > node.start {
                best = self.measured(point, node, best, unit)?;
                continue;
            }
            let left = at.checked_add(1)?;
            let right = self.nodes.get(left)?.after;
            let nearer = gap(point, self.nodes.get(left)?.bounds) <= gap(point, self.nodes.get(right)?.bounds);
            for next in if nearer { [right, left] } else { [left, right] } {
                *stack.get_mut(held)? = next;
                held = held.checked_add(1)?;
            }
        }
        Some(best)
    }

    /// The least of `best` and the measures from `point` to a leaf's edges
    /// whose envelopes lie within reach of the best so far.
    fn measured(&self, point: Point, leaf: &Node, best: f64, unit: f64) -> Option<f64> {
        let edges = self.edges.get(leaf.start..leaf.end)?;
        Some(edges.iter().fold(
            best,
            |best, &(a, b)| {
                if beyond(point, envelope(a, b), reach(best, unit)) { best } else { best.min(segment(point, a, b)) }
            },
        ))
    }

    /// Calls `visit` with each edge whose envelope meets the closed
    /// `window`, until it returns false; none if the hierarchy is
    /// inconsistent.
    pub(super) fn within(&self, window: Window, mut visit: impl FnMut(Point, Point) -> bool) -> Option<()> {
        let mut at = 0_usize;
        while let Some(node) = self.nodes.get(at) {
            if !meets(node.bounds, window) {
                at = node.after;
            } else if node.end > node.start {
                let edges = self.edges.get(node.start..node.end)?;
                let finished = edges.iter().filter(|&&(a, b)| meets(envelope(a, b), window)).all(|&(a, b)| visit(a, b));
                if !finished {
                    return Some(());
                }
                at = node.after;
            } else {
                at = at.checked_add(1)?;
            }
        }
        Some(())
    }
}

/// Builds the subtree of `edges`, which begin at `start` in leaf order.
fn build(edges: &mut [(Point, Point)], start: usize, nodes: &mut Vec<Node>, depth: u32) -> Option<()> {
    if depth >= MAX_DEPTH {
        return None;
    }
    let bounds = edges.iter().map(|&(a, b)| envelope(a, b)).reduce(union)?;
    let at = nodes.len();
    let leaf = edges.len() <= LEAF;
    let end = if leaf { start.checked_add(edges.len())? } else { start };
    nodes.push(Node { bounds, after: 0, start, end });
    if !leaf {
        let vertical = axis(edges)?;
        let middle = edges.len().checked_div(2)?;
        edges.select_nth_unstable_by(middle, |&a, &b| order(a, b, vertical));
        let (left, right) = edges.split_at_mut_checked(middle)?;
        build(left, start, nodes, depth.checked_add(1)?)?;
        build(right, start.checked_add(middle)?, nodes, depth.checked_add(1)?)?;
    }
    let after = nodes.len();
    nodes.get_mut(at)?.after = after;
    Some(())
}

/// Whether to split `edges` at their median along y rather than x: the
/// axis whose halves' envelopes overlap least, then whose halves'
/// envelopes have the least perimeter, then x. Leaves `edges` reordered.
fn axis(edges: &mut [(Point, Point)]) -> Option<bool> {
    let middle = edges.len().checked_div(2)?;
    let mut costs = [(0.0_f64, 0.0_f64); 2];
    for (cost, vertical) in costs.iter_mut().zip([false, true]) {
        edges.select_nth_unstable_by(middle, |&a, &b| order(a, b, vertical));
        let (left, right) = edges.split_at_checked(middle)?;
        let (left, right) = (bounds_of(left)?, bounds_of(right)?);
        *cost = (overlap(left, right), perimeter(left) + perimeter(right));
    }
    let [across, along] = costs;
    Some(along.0.total_cmp(&across.0).then(along.1.total_cmp(&across.1)) == Ordering::Less)
}

/// Edges in order of their centres along y, if `vertical`, or x, and of
/// their centres along the other axis where those are equal.
fn order(a: (Point, Point), b: (Point, Point), vertical: bool) -> Ordering {
    centre(a, vertical).total_cmp(&centre(b, vertical)).then(centre(a, !vertical).total_cmp(&centre(b, !vertical)))
}

/// The envelope of some edges.
fn bounds_of(edges: &[(Point, Point)]) -> Option<Window> {
    edges.iter().map(|&(a, b)| envelope(a, b)).reduce(union)
}

/// The area two envelopes share, as computed.
fn overlap([ax0, ay0, ax1, ay1]: Window, [bx0, by0, bx1, by1]: Window) -> f64 {
    let width = (ax1.min(bx1) - ax0.max(bx0)).max(0.0_f64);
    let height = (ay1.min(by1) - ay0.max(by0)).max(0.0_f64);
    width * height
}

/// Half an envelope's perimeter, as computed.
fn perimeter([x0, y0, x1, y1]: Window) -> f64 {
    (x1 - x0) + (y1 - y0)
}

/// Whether an envelope lies beyond `reach` of `point`: outside the square
/// of that half-side about it, an exact test, or by a squared distance
/// beyond the squared reach by more than that distance's rounding.
fn beyond(point: Point, bounds: Window, reach: f64) -> bool {
    !meets(bounds, around(point, reach)) || gap(point, bounds) > reach * reach * SQUARED_SLACK
}

/// An edge's centre along y, if `vertical`, or x; halved before adding so
/// extreme finite coordinates cannot overflow.
fn centre(([ax, ay], [bx, by]): (Point, Point), vertical: bool) -> f64 {
    if vertical { ay * HALF + by * HALF } else { ax * HALF + bx * HALF }
}

/// The square of half-side `reach` about `point`, rounded outward.
fn around([px, py]: Point, reach: f64) -> Window {
    [(px - reach).next_down(), (py - reach).next_down(), (px + reach).next_up(), (py + reach).next_up()]
}

/// The squared distance from `point` to an envelope, as computed: it
/// orders the search, and prunes it only past `SQUARED_SLACK`.
fn gap([px, py]: Point, [x0, y0, x1, y1]: Window) -> f64 {
    let dx = (x0 - px).max(px - x1).max(0.0_f64);
    let dy = (y0 - py).max(py - y1).max(0.0_f64);
    dx * dx + dy * dy
}

pub(super) fn envelope([ax, ay]: Point, [bx, by]: Point) -> Window {
    [ax.min(bx), ay.min(by), ax.max(bx), ay.max(by)]
}

fn union([ax0, ay0, ax1, ay1]: Window, [bx0, by0, bx1, by1]: Window) -> Window {
    [ax0.min(bx0), ay0.min(by0), ax1.max(bx1), ay1.max(by1)]
}

/// Whether two closed envelopes share a point.
fn meets([ax0, ay0, ax1, ay1]: Window, [bx0, by0, bx1, by1]: Window) -> bool {
    ax0 <= bx1 && bx0 <= ax1 && ay0 <= by1 && by0 <= ay1
}

#[cfg(test)]
mod tests;
