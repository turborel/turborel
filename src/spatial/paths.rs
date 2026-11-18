//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §13.1] Shortest paths over a
//! table of paths: a graph whose nodes are the end points of the paths'
//! curves and whose edges are the paths, each of a weight, searched by
//! Dijkstra's algorithm with a binary heap (`dijkstra.rs`). Every shortest
//! path from the start to the end is then found among the edges that lie
//! on one (`enumerate.rs`), as Description 11 returns each of them.
//!
//! Readings. A path passes no node twice, so a cycle of weight zero adds
//! no path. Two paths are of the same total length when their totals,
//! each summed from the start in double precision, are the same number.
//! A path from a point to itself is the path of no edge, of weight 0.

mod dijkstra;
mod enumerate;
pub mod sql;

use crate::error::Result;

/// An edge of the graph: a path of the table, walked from one node to
/// another.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Edge {
    /// The node it leaves.
    pub from: usize,
    /// The node it reaches.
    pub to: usize,
    /// Its weight, finite and not negative.
    pub weight: f64,
    /// The path's identifier.
    pub id: i32,
}

/// A graph of `nodes` nodes, numbered from 0, and its edges.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Graph {
    /// How many nodes it has.
    pub nodes: usize,
    /// Its edges.
    pub edges: Vec<Edge>,
}

impl Graph {
    /// The edges of each node, by their position in `edges`: those that
    /// leave it if `leaving`, else those that reach it.
    fn edges_of(&self, leaving: bool) -> Vec<Vec<usize>> {
        let mut out = vec![Vec::new(); self.nodes];
        for (k, e) in self.edges.iter().enumerate() {
            let node = if leaving { e.from } else { e.to };
            if let Some(list) = out.get_mut(node) {
                list.push(k);
            }
        }
        out
    }
}

/// The shortest paths from one node to another: their total weight and
/// each one's edges' identifiers, in order, the paths in ascending order
/// of those.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    /// The total weight of each.
    pub total: f64,
    /// The paths.
    pub paths: Vec<Vec<i32>>,
}

/// [§13.1.1 Description 4, 5, 10, 11] Every shortest path of `graph`
/// from node `start` to node `end`; none if no path leads there.
///
/// # Errors
///
/// SQLCODE -900 if there are more such paths than a table function
/// returns, or more to search than a search takes (`enumerate.rs`).
pub fn shortest(graph: &Graph, start: usize, end: usize) -> Result<Option<Found>> {
    let best = dijkstra::distances(graph, start);
    let Some(total) = best.get(end).copied().flatten() else { return Ok(None) };
    let mut paths = enumerate::all_paths(graph, &best, (start, end))?;
    paths.sort();
    Ok(Some(Found { total, paths }))
}

#[cfg(test)]
mod tests;
