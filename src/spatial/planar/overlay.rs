//! The point-set operations ST_Intersection, ST_Union, ST_Difference
//! and ST_SymDifference [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.31,
//! §5.1.33, §5.1.35, §5.1.37], over operands of any dimensions. Both
//! operands are noded into one graph. A face is in the result when the
//! operation keeps its two area locations; an edge or node when it keeps
//! the edge's or node's own locations. The closure the clauses require
//! follows. An edge beside a kept face is part of that surface, and a
//! node touching a kept face or edge is part of it, so only edges and
//! nodes with nothing kept around them are output on their own. The
//! result's type is the most specific for what remains, one of the sets
//! of §5.1.39 Tables 7 to 11.

use super::assemble::{Selection, assemble};
use super::graph::{Graph, INPUTS};
use super::label;
use super::operand::paths;
use super::shape::Geometry;
use crate::error::Result;

/// A point-set operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    /// Closure(a ∩ b) [§5.1.31].
    Intersection,
    /// Closure(a ∪ b) [§5.1.33].
    Union,
    /// Closure(a − b) [§5.1.35].
    Difference,
    /// Closure(Closure(a − b) ∪ Closure(b − a)) [§5.1.37].
    SymDifference,
}

impl Operation {
    /// Whether a position in the first operand or not, and in the second
    /// or not, is in the result.
    pub(crate) fn keeps(self, [first, second]: [bool; 2]) -> bool {
        match self {
            Self::Intersection => first && second,
            Self::Union => first || second,
            Self::Difference => first && !second,
            Self::SymDifference => first != second,
        }
    }
}

/// The result of an operation on two geometries.
///
/// # Errors
///
/// SQLCODE -910 if noding does not settle or the result's rings do not
/// close.
pub fn overlay(first: &Geometry, second: &Geometry, operation: Operation) -> Result<Geometry> {
    let (one, two) = (first.parts(), second.parts());
    if one.is_empty() || two.is_empty() {
        return Ok(with_empty((first, one.is_empty()), (second, two.is_empty()), operation));
    }
    let mut all = paths(&one, 0);
    all.extend(paths(&two, 1));
    let graph = Graph::build(&all)?;
    assemble(&graph, &select(&graph, operation))
}

/// An empty operand leaves the other, or nothing, as the operation
/// keeps it; Closure of a closed set is itself.
fn with_empty((first, first_empty): (&Geometry, bool), (second, second_empty): (&Geometry, bool), operation: Operation) -> Geometry {
    if !first_empty && operation.keeps([true, false]) {
        first.clone()
    } else if !second_empty && operation.keeps([false, true]) {
        second.clone()
    } else {
        Geometry::Empty
    }
}

/// The faces, edges and nodes an operation keeps.
pub(crate) fn select(graph: &Graph, operation: Operation) -> Selection {
    let second = INPUTS.saturating_sub(1);
    let faces: Vec<bool> = graph
        .cycles
        .iter()
        .map(|cycle| {
            let [one, two] = cycle.depth;
            operation.keeps([one > 0_i32, two > 0_i32])
        })
        .collect();
    let mut selection = Selection { faces, lines: Vec::new(), points: Vec::new() };
    let lines: Vec<bool> = (0..graph.edges.len())
        .map(|id| {
            let kept = operation.keeps([label::edge(graph, id, 0).inside(), label::edge(graph, id, second).inside()]);
            let forward = id.saturating_mul(2);
            let beside = [forward, forward.saturating_add(1)]
                .iter()
                .any(|half| graph.face.get(*half).and_then(|cycle| selection.faces.get(*cycle)).copied().unwrap_or_default());
            kept && !beside
        })
        .collect();
    selection.lines = lines;
    let points: Vec<bool> = (0..graph.nodes.len())
        .map(|id| {
            let kept = operation.keeps([label::node(graph, id, 0).inside(), label::node(graph, id, second).inside()]);
            kept && !selection.covers(graph, id) && !enclosed(graph, id, operation)
        })
        .collect();
    selection.points = points;
    selection
}

/// Whether an isolated node lies in a face the operation keeps, the
/// face around its component.
fn enclosed(graph: &Graph, node: usize, operation: Operation) -> bool {
    let isolated = graph.star.get(node).is_none_or(Vec::is_empty);
    let around = graph.component.get(node).and_then(|component| graph.outside.get(*component));
    isolated && around.is_some_and(|[one, two]| operation.keeps([*one > 0_i32, *two > 0_i32]))
}

/// ST_Intersection [§5.1.31].
///
/// # Errors
///
/// As [`overlay`].
pub fn intersection(first: &Geometry, second: &Geometry) -> Result<Geometry> {
    overlay(first, second, Operation::Intersection)
}

/// ST_Union [§5.1.33].
///
/// # Errors
///
/// As [`overlay`].
pub fn union(first: &Geometry, second: &Geometry) -> Result<Geometry> {
    overlay(first, second, Operation::Union)
}

/// ST_Difference [§5.1.35].
///
/// # Errors
///
/// As [`overlay`].
pub fn difference(first: &Geometry, second: &Geometry) -> Result<Geometry> {
    overlay(first, second, Operation::Difference)
}

/// ST_SymDifference [§5.1.37].
///
/// # Errors
///
/// As [`overlay`].
pub fn sym_difference(first: &Geometry, second: &Geometry) -> Result<Geometry> {
    overlay(first, second, Operation::SymDifference)
}
