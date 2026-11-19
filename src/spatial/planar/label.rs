//! \[DESIGN\] Where each node, edge and face of the graph lies in each
//! operand, its interior, boundary or exterior [ISO/IEC 13249-3:2016
//! (DIS, SC32 N2593) §4.2.2]. A surface's interior is where its area
//! depth is positive and its boundary separates positive from zero
//! depth. A curve's boundary is the mod 2 union of its elements' ends
//! [§4.2.2, §10.3.1 Description 5]; the rest of it is interior, as is
//! every point. Where parts of different dimensions meet, the higher
//! dimension decides. A curve end inside a surface is interior to the
//! collection, as the point set of the union has it.

use super::graph::{Graph, edge_of, twin};

/// A position's place relative to a geometry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Location {
    /// In the interior.
    Interior,
    /// On the boundary.
    Boundary,
    /// In the exterior.
    Exterior,
}

impl Location {
    /// The row or column of the DE-9IM this location selects.
    pub(crate) fn index(self) -> usize {
        match self {
            Self::Interior => 0,
            Self::Boundary => 1,
            Self::Exterior => 2,
        }
    }

    /// Whether the position belongs to the closed point set.
    pub(crate) fn inside(self) -> bool {
        self != Self::Exterior
    }
}

/// A face's place in an operand, from its area depth.
pub(crate) fn face(depth: i32) -> Location {
    if depth > 0 { Location::Interior } else { Location::Exterior }
}

/// An edge's place in an operand.
pub(crate) fn edge(graph: &Graph, id: usize, input: usize) -> Location {
    let forward = id.saturating_mul(2);
    let left = graph.left_depth(forward, input) > 0_i32;
    let right = graph.left_depth(twin(forward), input) > 0_i32;
    let lines = graph.edges.get(id).and_then(|edge| edge.lines.get(input).copied()).unwrap_or_default();
    match (left, right) {
        (true, true) => Location::Interior,
        (true, false) | (false, true) => Location::Boundary,
        (false, false) => {
            if lines > 0 {
                Location::Interior
            } else {
                Location::Exterior
            }
        }
    }
}

/// A node's place in an operand, from the faces around it if a surface
/// reaches it, else from the curves and points there.
pub(crate) fn node(graph: &Graph, id: usize, input: usize) -> Location {
    let star = graph.star.get(id).map_or(&[][..], Vec::as_slice);
    let depths: Vec<bool> = if star.is_empty() {
        let component = graph.component.get(id).copied().unwrap_or_default();
        vec![graph.outside.get(component).and_then(|depth| depth.get(input)).is_some_and(|depth| *depth > 0_i32)]
    } else {
        star.iter().map(|half| graph.left_depth(*half, input) > 0_i32).collect()
    };
    if depths.iter().all(|inside| *inside) {
        return Location::Interior;
    }
    if depths.iter().any(|inside| *inside) {
        return Location::Boundary;
    }
    let Some(node) = graph.nodes.get(id) else { return Location::Exterior };
    if node.ends.get(input).is_some_and(|ends| ends & 1 == 1) {
        return Location::Boundary;
    }
    let on_line =
        star.iter().any(|half| graph.edges.get(edge_of(*half)).and_then(|edge| edge.lines.get(input)).is_some_and(|lines| *lines > 0));
    if on_line || node.point.get(input).copied().unwrap_or_default() { Location::Interior } else { Location::Exterior }
}
