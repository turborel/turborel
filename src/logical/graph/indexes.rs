//! \[EXTENSION\] Which label and reachability indexes a GRAPH_TABLE reads
//! (RULES.md D115, D116), decided of its bound form alone, so EXPLAIN
//! names those the search will read.
//!
//! A path pattern's starts are pruned by a reachability index when every
//! edge of a match of it is of one edge table, all followed one way,
//! between vertices of one vertex table, which both its ends reference by
//! the same columns, and its last node pattern has a WHERE of its own
//! element alone: a start of that table reaching no vertex of it that
//! WHERE keeps, nor being one, has no match, as a match of an edge ends
//! in the table and one of none ends at its start. It is read only for a
//! pattern with a quantifier, a search that may walk far.

use std::fmt::Write as _;

use super::{GraphMatch, PathShape, PatternShape, TestShape};
use crate::graph::view::Orientation;

/// \[EXTENSION\] How a path pattern's starts are pruned (RULES.md D116).
#[derive(Debug, Clone)]
pub struct Pruning {
    /// The edge table every edge of a match is of.
    pub edges: usize,
    /// The vertex table its edges join, of the starts it prunes.
    pub vertices: usize,
    /// Whether its edges are followed from destination to source.
    pub reverse: bool,
    /// The node pattern its matches end at.
    pub end: TestShape,
    /// The prefilter of that node pattern.
    pub filter: usize,
    /// The reachability index of the edge table.
    pub index: String,
}

/// Pushes the edge patterns of `shape` onto `out`; whether it has a
/// quantifier.
fn edges_of<'s>(shape: &'s PatternShape, out: &mut Vec<(&'s TestShape, Orientation)>) -> bool {
    match shape {
        PatternShape::Vertex(_) => false,
        PatternShape::Edge(t, o) => {
            out.push((t, *o));
            false
        }
        PatternShape::Concat(parts) | PatternShape::Union(parts) | PatternShape::Alternation(parts) => {
            // Each part's edges are pushed, whichever has a quantifier.
            let mut quantified = false;
            for p in parts {
                quantified |= edges_of(p, out);
            }
            quantified
        }
        PatternShape::Quantified(body, _, _) => {
            edges_of(body, out);
            true
        }
        PatternShape::Group(_, body, _) => edges_of(body, out),
    }
}

/// The node pattern every match of `shape` ends with, if one does.
fn last_vertex(shape: &PatternShape) -> Option<&TestShape> {
    match shape {
        PatternShape::Vertex(t) => Some(t),
        PatternShape::Concat(parts) => parts.last().and_then(last_vertex),
        PatternShape::Group(_, body, _) => last_vertex(body),
        PatternShape::Edge(..) | PatternShape::Union(_) | PatternShape::Alternation(_) | PatternShape::Quantified(..) => None,
    }
}

impl GraphMatch {
    /// The element tables element variable `shape` may bind.
    fn bindable(&self, shape: usize) -> Vec<usize> {
        self.vars.get(shape).map(|v| v.tables.iter().map(|(t, _)| *t).collect()).unwrap_or_default()
    }

    /// The one edge table, and the way, every edge of `edges` is of.
    fn one_way(&self, edges: &[(&TestShape, Orientation)]) -> Option<(usize, bool)> {
        let mut tables: Vec<usize> = edges.iter().flat_map(|(t, _)| self.bindable(t.shape)).collect();
        tables.sort_unstable();
        tables.dedup();
        let [table] = tables.as_slice() else { return None };
        let right = edges.iter().all(|(_, o)| *o == Orientation::RIGHT);
        let left = edges.iter().all(|(_, o)| *o == Orientation::LEFT);
        (right || left).then_some((*table, left))
    }

    /// \[EXTENSION\] How `path`'s starts are pruned by a reachability
    /// index, if they are (RULES.md D116).
    pub fn pruning(&self, path: &PathShape) -> Option<Pruning> {
        let mut edges = Vec::new();
        if !edges_of(&path.pattern, &mut edges) {
            return None;
        }
        let (table, reverse) = self.one_way(&edges)?;
        let source = self.tables.get(table)?;
        let ends = source.ends.as_ref().filter(|e| e.directed)?;
        let vertices = ends.source.table;
        let alike = ends.destination.table == vertices && ends.source.references == ends.destination.references;
        let end = last_vertex(&path.pattern)?;
        let filter = end.filter?;
        let own = self.prefilters.get(filter).is_some_and(|p| p.own == Some(end.shape) && p.reads.is_empty());
        if !(alike && own && path.starts.contains(&vertices)) {
            return None;
        }
        let index = source.reachability.clone()?;
        Some(Pruning { edges: table, vertices, reverse, end: end.clone(), filter, index })
    }

    /// \[GS §16 R16.9\] The lines of EXPLAIN naming the label and
    /// reachability indexes read.
    pub(super) fn render_indexes(&self, out: &mut String) {
        for (n, path) in self.paths.iter().enumerate() {
            if let Some(p) = self.pruning(path) {
                let _ = write!(out, "\n  path {}: starts pruned by reachability index {}", n.saturating_add(1), p.index);
            }
        }
        for t in &self.tables {
            if let Some(ix) = &t.labels_index {
                let _ = write!(out, "\n  element table {}: labels tested by label index {ix}", t.alias);
            }
        }
    }
}
