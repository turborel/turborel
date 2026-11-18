//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §13.1] Dijkstra's algorithm
//! with a binary heap: the least total weight from one node to each node
//! a path leads to. Weights are not negative, so a node taken from the
//! heap has its least weight and is settled; a node is pushed again only
//! when its weight falls, so the search ends after each edge is relaxed
//! once.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use super::Graph;

/// A node waiting in the heap, at the total weight it was reached by.
#[derive(Debug, Clone, Copy)]
struct Waiting {
    /// The total weight.
    weight: f64,
    /// The node.
    node: usize,
}

/// The heap gives the greatest first, so a waiting node is greater the
/// less its weight; the node's number breaks a tie, so the order is total
/// and the search the same each time.
impl Ord for Waiting {
    fn cmp(&self, other: &Self) -> Ordering {
        other.weight.total_cmp(&self.weight).then_with(|| other.node.cmp(&self.node))
    }
}

impl PartialOrd for Waiting {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Waiting {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Waiting {}

/// Whether `found` is less than the best weight known, `known`.
fn less(found: f64, known: Option<f64>) -> bool {
    known.is_none_or(|k| found.total_cmp(&k) == Ordering::Less)
}

/// The least total weight of a path from `start` to each node of
/// `graph`, none of a node no path reaches.
pub(super) fn distances(graph: &Graph, start: usize) -> Vec<Option<f64>> {
    let leaving = graph.edges_of(true);
    let mut best: Vec<Option<f64>> = vec![None; graph.nodes];
    let mut settled = vec![false; graph.nodes];
    let mut heap = BinaryHeap::new();
    if let Some(b) = best.get_mut(start) {
        *b = Some(0.0_f64);
        heap.push(Waiting { weight: 0.0_f64, node: start });
    }
    while let Some(Waiting { weight, node }) = heap.pop() {
        let Some(done) = settled.get_mut(node) else { continue };
        if *done {
            continue;
        }
        *done = true;
        for k in leaving.get(node).into_iter().flatten() {
            let Some(e) = graph.edges.get(*k) else { continue };
            let reached = weight + e.weight;
            let Some(b) = best.get_mut(e.to) else { continue };
            if less(reached, *b) {
                *b = Some(reached);
                heap.push(Waiting { weight: reached, node: e.to });
            }
        }
    }
    best
}
