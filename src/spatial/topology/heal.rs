//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.10, §11.3.11]
//! ST_NewEdgeHeal and ST_ModEdgeHeal: two edges joined at a node no other
//! edge ends at, that node removed, into one edge in the direction of the
//! first: a new edge, or the first edge itself.

use super::split::coords_of;
use super::{Edge, Topology, damaged, fault, linestring};
use crate::error::{Result, SqlState};
use crate::geometry::model::{Coords, Dims};

/// The rule of ST_NewEdgeHeal's checks.
const NEW: &str = "§11.3.10 Description 2f";
/// The rule of ST_ModEdgeHeal's checks.
const MODIFY: &str = "§11.3.11 Description 2f";

/// [Description 2f iii 5] How the two edges meet: the common node is the
/// first's end and the second's start (1), both ends (2), both starts (3),
/// or the first's start and the second's end (4).
#[derive(Debug, Clone, Copy)]
enum Meet {
    EndStart,
    EndEnd,
    StartStart,
    StartEnd,
}

/// How edges `e1` and `e2` meet, and at which node; none if they do not.
fn meet(e1: &Edge, e2: &Edge) -> Option<(Meet, i32)> {
    if e1.end == e2.start {
        Some((Meet::EndStart, e1.end))
    } else if e1.end == e2.end {
        Some((Meet::EndEnd, e1.end))
    } else if e1.start == e2.start {
        Some((Meet::StartStart, e1.start))
    } else if e1.start == e2.end {
        Some((Meet::StartEnd, e1.start))
    } else {
        None
    }
}

/// The points of `c`, of `dims`, in reverse order.
fn reversed(c: &Coords, dims: Dims) -> Coords {
    let points: Vec<&[f64]> = c.chunks_exact(dims.stride()).collect();
    points.into_iter().rev().flatten().copied().collect()
}

/// `a` then `b` without `b`'s first point, which is `a`'s last.
fn joined(a: Coords, b: &Coords, dims: Dims) -> Coords {
    let mut out = a;
    out.extend(b.iter().skip(dims.stride()));
    out
}

impl Topology {
    /// [§11.3.10, §11.3.11] Edges `first` and `second` healed into one: a
    /// new edge if `new_edge`, whose identifier is returned, else `first`.
    /// Reading: an edge healed with itself is SQLSTATE 2FF40, as it is no
    /// pair of edges to connect.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF43 (non-existent edge); 2FF40 (non-connected edges);
    /// 2FF52 (other edges connected); 2FF25 (mixed coordinate dimensions)
    /// if the two curves' dimensions differ.
    pub fn heal(&mut self, first: i32, second: i32, new_edge: bool) -> Result<Option<i32>> {
        let rule = if new_edge { NEW } else { MODIFY };
        let e1 = self.edge(first, &format!("{rule} i"))?.clone();
        let e2 = self.edge(second, &format!("{rule} ii"))?.clone();
        let found = if first == second { None } else { meet(&e1, &e2) };
        let (how, common) = found.ok_or_else(|| {
            fault(&format!("{rule} iii 5E"), SqlState::SPATIAL_NON_CONNECTED_EDGES, &format!("edges {first} and {second} share no node"))
        })?;
        if self.has_edges(common, &[first, second]) {
            return Err(fault(
                &format!("{rule} iv"),
                SqlState::SPATIAL_OTHER_EDGES_CONNECTED,
                &format!("another edge ends at node {common}"),
            ));
        }
        let dims = e1.geometry.dims;
        if e2.geometry.dims != dims {
            return Err(fault(
                &format!("{rule} v"),
                SqlState::SPATIAL_MIXED_COORDINATE_DIMENSIONS,
                "the two curves have different coordinate dimensions",
            ));
        }
        let (c1, c2) = (coords_of(&e1.geometry)?, coords_of(&e2.geometry)?);
        let (start, end, coords) = match how {
            Meet::EndStart => (e1.start, e2.end, joined(c1, &c2, dims)),
            Meet::EndEnd => (e1.start, e2.start, joined(c1, &reversed(&c2, dims), dims)),
            Meet::StartStart => (e2.end, e1.end, joined(reversed(&c2, dims), &c1, dims)),
            Meet::StartEnd => (e2.start, e1.end, joined(c2, &c1, dims)),
        };
        let geometry = linestring(coords, (e1.geometry.srid, dims))?;
        // A new identifier, neither of the healed edges'.
        let id = if new_edge { self.new_edge_id()? } else { first };
        self.nodes.remove(&common);
        self.edges.remove(&second);
        if new_edge {
            self.edges.remove(&first);
        }
        self.edges.insert(id, Edge { start, end, geometry, ..e1 });
        self.relink(&[start, end])?;
        if !self.edges.contains_key(&id) {
            return Err(damaged("the healed edge is not there"));
        }
        Ok(new_edge.then_some(id))
    }
}
