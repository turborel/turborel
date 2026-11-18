//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.1 Description 2e iii]
//! Where a point lies in a topology: the face whose geometry it is
//! within, the universal face if none, and the checks the routines share
//! on a point or a curve against the nodes and edges there.

use super::predicates::{Side, inside_line, on_line, same};
use super::{Point, Topology, UNIVERSAL_FACE, fault, line_of, point_of};
use crate::error::{Result, SqlState};

impl Topology {
    /// The face whose geometry `p` is within; the universal face if it is
    /// within none. A point on an edge is within no face's interior, so it
    /// is given the universal face too; the routines refuse such a point
    /// first.
    pub(super) fn face_at(&self, p: Point) -> Result<i32> {
        for (face, shape) in self.shapes()? {
            if shape.side(p) == Side::Inside {
                return Ok(face);
            }
        }
        Ok(UNIVERSAL_FACE)
    }

    /// A node at `p` but `except`, if there is one.
    pub(super) fn node_at(&self, p: Point, except: Option<i32>) -> Result<Option<i32>> {
        for (id, node) in &self.nodes {
            if Some(*id) != except && same(point_of(&node.geometry)?, p) {
                return Ok(Some(*id));
            }
        }
        Ok(None)
    }

    /// [§11.3.1 Description 2e i, §11.3.2 Description 2f i] That no node
    /// but `except` is at `p`.
    pub(super) fn no_coincident_node(&self, p: Point, except: Option<i32>, rule: &str) -> Result<()> {
        match self.node_at(p, except)? {
            Some(n) => Err(fault(rule, SqlState::SPATIAL_COINCIDENT_NODE, &format!("node {n} is there"))),
            None => Ok(()),
        }
    }

    /// [§11.3.1 Description 2e ii] That no edge passes through `p`.
    /// Reading: ST_Crosses of a curve and a point is not defined [§5.1.51],
    /// so an edge "crosses" a node it passes through.
    pub(super) fn no_edge_through(&self, p: Point, rule: &str) -> Result<()> {
        for (id, edge) in &self.edges {
            if on_line(p, &line_of(&edge.geometry)?) {
                return Err(fault(rule, SqlState::SPATIAL_EDGE_CROSSES_NODE, &format!("edge {id} passes through the point")));
            }
        }
        Ok(())
    }

    /// [§11.3.4 Description 2h ix] That the line `line` passes through no
    /// isolated node but at its ends.
    pub(super) fn no_isolated_node_on(&self, line: &[Point], rule: &str) -> Result<()> {
        for (id, node) in &self.nodes {
            if self.has_edges(*id, &[]) {
                continue;
            }
            if inside_line(point_of(&node.geometry)?, line) {
                return Err(fault(rule, SqlState::SPATIAL_GEOMETRY_CROSSES_A_NODE, &format!("the curve passes through node {id}")));
            }
        }
        Ok(())
    }

    /// Whether node `id` is the end of no edge.
    pub(super) fn isolated(&self, id: i32) -> bool {
        !self.has_edges(id, &[])
    }
}
