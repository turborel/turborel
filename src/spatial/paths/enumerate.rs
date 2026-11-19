//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §13.1.1 Description 11] Every
//! shortest path: an edge lies on one when the least weight of its end is
//! the least weight of its start and its own, summed as the search summed
//! them, so the shortest paths to the end are the walks back from the end
//! to the start over such edges. The walk keeps a stack, not recursion,
//! and passes no node twice, so it ends.
//!
//! Implementation-defined limits: at most `MAX_PATHS` paths are returned
//! and at most `MAX_STEPS` steps are walked; past either the search fails
//! rather than return some of the paths as all of them.

use std::cmp::Ordering;

use super::Graph;
use crate::error::{Code, Result, SqlError};
use crate::value::types::MAX_ARRAY_CARDINALITY;

/// The most shortest paths one search returns: as many as an array has
/// elements at most.
pub const MAX_PATHS: u32 = MAX_ARRAY_CARDINALITY;

/// The most steps of the walk back from the end, each over one edge or
/// back from one node.
pub const MAX_STEPS: u64 = 1 << 26;

/// Whether `a` and `b` are the same weight. Both are sums the search made
/// in the same order of the same numbers, so equal sums are the same bits
/// [§13.1.1 Description 11].
fn same(a: f64, b: f64) -> bool {
    a.total_cmp(&b) == Ordering::Equal
}

/// The edges that reach each node and lie on a shortest path to it, of
/// the least weights `best`.
fn tight(graph: &Graph, best: &[Option<f64>]) -> Vec<Vec<usize>> {
    let reaching = graph.edges_of(false);
    let weight = |n: usize| best.get(n).copied().flatten();
    reaching
        .into_iter()
        .map(|list| {
            list.into_iter()
                .filter(|k| {
                    let Some(e) = graph.edges.get(*k) else { return false };
                    match (weight(e.from), weight(e.to)) {
                        (Some(f), Some(t)) => same(f + e.weight, t),
                        (None, _) | (_, None) => false,
                    }
                })
                .collect()
        })
        .collect()
}

/// The error of a search past a limit.
fn too_many(what: &str) -> SqlError {
    SqlError::new(Code::Unsupported, format!("[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §13.1.1 Description 11] {what}"))
}

/// The search's walk back: the nodes on the walk, each with the next
/// edge to try; the edges walked; and which nodes are on the walk.
struct Walk {
    /// Each node of the walk, from the end, and the next of its edges to
    /// try.
    frames: Vec<(usize, usize)>,
    /// The edges walked, from the end back.
    edges: Vec<usize>,
    /// Whether each node is on the walk.
    on: Vec<bool>,
}

impl Walk {
    /// The walk back from the last node.
    fn back(&mut self) {
        if let Some((node, _)) = self.frames.pop()
            && let Some(on) = self.on.get_mut(node)
        {
            *on = false;
        }
        self.edges.pop();
    }

    /// The walk on over edge `k` to node `from`, unless it is on the walk.
    fn on_to(&mut self, k: usize, from: usize) {
        let Some(on) = self.on.get_mut(from) else { return };
        if *on {
            return;
        }
        *on = true;
        self.edges.push(k);
        self.frames.push((from, 0));
    }
}

/// Every shortest path of `graph` from `start` to `end`, of the least
/// weights `best`, each the identifiers of its edges in order.
///
/// # Errors
///
/// SQLCODE -900 past `MAX_PATHS` paths or `MAX_STEPS` steps.
pub(super) fn all_paths(graph: &Graph, best: &[Option<f64>], (start, end): (usize, usize)) -> Result<Vec<Vec<i32>>> {
    let tight = tight(graph, best);
    let mut walk = Walk { frames: vec![(end, 0)], edges: Vec::new(), on: vec![false; graph.nodes] };
    if let Some(on) = walk.on.get_mut(end) {
        *on = true;
    }
    let mut found = Vec::new();
    let mut steps = 0_u64;
    while let Some((node, next)) = walk.frames.last_mut() {
        steps = steps.saturating_add(1);
        if steps > MAX_STEPS {
            return Err(too_many(&format!("finding the shortest paths takes more than {MAX_STEPS} steps")));
        }
        if *node == start {
            let ids = walk.edges.iter().rev().filter_map(|k| graph.edges.get(*k)).map(|e| e.id);
            found.push(ids.collect::<Vec<i32>>());
            if found.len() > usize::try_from(MAX_PATHS).unwrap_or(usize::MAX) {
                return Err(too_many(&format!("there are more than {MAX_PATHS} shortest paths")));
            }
            walk.back();
            continue;
        }
        let Some(k) = tight.get(*node).and_then(|list| list.get(*next)).copied() else {
            walk.back();
            continue;
        };
        *next = next.saturating_add(1);
        let from = graph.edges.get(k).map_or(start, |e| e.from);
        walk.on_to(k, from);
    }
    Ok(found)
}
