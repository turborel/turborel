//! A prepared polygon's edges in a static hierarchy of their envelopes.
//! Each node splits its edges at the median along the axis on which their
//! centres spread widest, not on alternate axes, so long parallel edges
//! such as a comb's teeth still fall into narrow leaves. Point location
//! asks it for the edges meeting one grid cell (`cells.rs`); every edge it
//! returns is still tested exactly (D85).

use std::sync::Arc;

use super::shapes::rings;
use crate::geometry::crossing::segments;
use crate::geometry::shape::{Point, Shape};
use crate::memory::{MemoryBudget, Reservation};

/// Edges per leaf: few exact tests per leaf, and few nodes per edge.
const LEAF: usize = 8;
/// A median split halves the edges, so no usize-sized count reaches this
/// depth.
const MAX_DEPTH: u32 = usize::BITS;
const HALF: f64 = 0.5;

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
        Some(Box::new(Self { edges, nodes, _held: held }))
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
                if !edges.iter().filter(|&&(a, b)| meets(envelope(a, b), window)).all(|&(a, b)| visit(a, b)) {
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
        let vertical = spread(edges, true) > spread(edges, false);
        let middle = edges.len().checked_div(2)?;
        edges.select_nth_unstable_by(middle, |&a, &b| centre(a, vertical).total_cmp(&centre(b, vertical)));
        let (left, right) = edges.split_at_mut_checked(middle)?;
        build(left, start, nodes, depth.checked_add(1)?)?;
        build(right, start.checked_add(middle)?, nodes, depth.checked_add(1)?)?;
    }
    let after = nodes.len();
    nodes.get_mut(at)?.after = after;
    Some(())
}

/// How far the edges' centres spread along y, if `vertical`, or x.
fn spread(edges: &[(Point, Point)], vertical: bool) -> f64 {
    let (low, high) = edges
        .iter()
        .map(|&edge| centre(edge, vertical))
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), c| (low.min(c), high.max(c)));
    high - low
}

/// An edge's centre along y, if `vertical`, or x; halved before adding so
/// extreme finite coordinates cannot overflow.
fn centre(([ax, ay], [bx, by]): (Point, Point), vertical: bool) -> f64 {
    if vertical { ay * HALF + by * HALF } else { ax * HALF + bx * HALF }
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
