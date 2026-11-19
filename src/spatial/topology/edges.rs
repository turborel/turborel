//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.4, §11.3.6, §11.3.7] The
//! routines of isolated edges and of an edge's geometry: ST_AddIsoEdge,
//! ST_ChangeEdgeGeom and ST_RemIsoEdge, and the checks of a new curve the
//! routines that add an edge share.

use super::predicates::{meet_except, same};
use super::rings::polygon;
use super::{Edge, Point, Topology, UNIVERSAL_FACE, fault, line_of};
use crate::error::{Result, SqlState};
use crate::geometry::engine::engine_of;
use crate::geometry::model::Geom;
use crate::spatial::planar::{self, Geometry};

/// [§11.3.4] The rule of ST_AddIsoEdge's checks.
const ADD: &str = "§11.3.4 Description 2h";
/// [§11.3.6] The rule of ST_ChangeEdgeGeom's checks.
const CHANGE: &str = "§11.3.6 Description 2g";
/// [§11.3.7] The rule of ST_RemIsoEdge's checks.
const REMOVE: &str = "§11.3.7 Description 2e";
/// [§5.1.45] The intersection matrix pattern of two values whose
/// interiors meet.
const INTERIORS_MEET: &str = "T********";

/// [§5.1.8] The points of curve `curve`, which is simple.
///
/// # Errors
///
/// SQLSTATE 2FF29 (curve not simple) if it is not, of the rule `rule`.
pub(super) fn simple_line(curve: &Geom, rule: &str) -> Result<Vec<Point>> {
    let line = line_of(curve)?;
    if !planar::is_simple(&Geometry::Line(line.clone()))? {
        return Err(fault(rule, SqlState::SPATIAL_CURVE_NOT_SIMPLE, "the curve is not simple"));
    }
    Ok(line)
}

impl Topology {
    /// [§11.3.4 Description 2h vii, viii] That `line` starts at node `start`
    /// and ends at node `end`.
    pub(super) fn ends_at(&self, line: &[Point], (start, end): (i32, i32), rule: &str) -> Result<(Point, Point)> {
        let (p1, p2) = (self.at(start)?, self.at(end)?);
        if !line.first().is_some_and(|p| same(*p, p1)) {
            return Err(fault(rule, SqlState::SPATIAL_START_NODE_NOT_GEOMETRY_START, &format!("the curve does not start at node {start}")));
        }
        if !line.last().is_some_and(|p| same(*p, p2)) {
            return Err(fault(rule, SqlState::SPATIAL_END_NODE_NOT_GEOMETRY_END, &format!("the curve does not end at node {end}")));
        }
        Ok((p1, p2))
    }

    /// [§11.3.12 Description 2h vi] That `line` meets no edge but `except`
    /// anywhere but at the points `allowed`, else `state` of `rule`.
    pub(super) fn meets_no_edge(
        &self,
        line: &[Point],
        (except, allowed): (Option<i32>, &[Point]),
        (rule, state): (&str, SqlState),
    ) -> Result<()> {
        for (id, edge) in &self.edges {
            if Some(*id) == except {
                continue;
            }
            if let Some([x, y]) = meet_except(line, &line_of(&edge.geometry)?, allowed) {
                return Err(fault(rule, state, &format!("the curve meets edge {id} at ({x}, {y})")));
            }
        }
        Ok(())
    }

    /// [§11.3.4 Description 2h vi] Whether `line` is within face `face`:
    /// within its geometry [§5.1.53], or, of the universal face, in the
    /// interior of no other face, as the universal face has no geometry.
    fn within_face(&self, line: &[Point], face: i32) -> Result<bool> {
        let curve = Geometry::Line(line.to_vec());
        for (f, shape) in self.shapes()? {
            let Some(g) = polygon(&shape, self.srid_or_unknown()) else { continue };
            let area = engine_of(&g)?;
            if f == face {
                return Ok(planar::within(&curve, &area)? == Some(true));
            }
            if face == UNIVERSAL_FACE && planar::relate_pattern(&curve, &area, INTERIORS_MEET)? == Some(true) {
                return Ok(false);
            }
        }
        Ok(face == UNIVERSAL_FACE)
    }

    /// [§11.3.4] ST_AddIsoEdge: a new edge of `curve` between isolated nodes
    /// `start` and `end`; its identifier. Reading: a closed curve from a
    /// node to itself bounds a new face, so it is added as ST_AddEdgeModFace
    /// adds one; the text's row of one face would leave the face it encloses
    /// out of the topology.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF29 (curve not simple); 2FF46 (non-existent node); 2FF50
    /// (not isolated node); 2FF39 (nodes in different faces); 2FF37
    /// (geometry not within face); 2FF57, 2FF33 (start or end node not the
    /// curve's); 2FF34 (geometry crosses a node); 2FF36 (geometry
    /// intersects an edge); 2FF10 of a curve of another system.
    pub fn add_iso_edge(&mut self, start: i32, end: i32, curve: &Geom) -> Result<i32> {
        self.same_srid(curve, "§11.3.4 Description 3")?;
        let line = simple_line(curve, "§11.3.4 Description 2g")?;
        let (n1, n2) = (self.node(start, &format!("{ADD} i"))?.clone(), self.node(end, &format!("{ADD} ii"))?.clone());
        for (id, at) in [(start, "iii"), (end, "iv")] {
            if !self.isolated(id) {
                return Err(fault(&format!("{ADD} {at}"), SqlState::SPATIAL_NOT_ISOLATED_NODE, &format!("an edge ends at node {id}")));
            }
        }
        let face = n1.containing_face.unwrap_or(UNIVERSAL_FACE);
        if n2.containing_face.unwrap_or(UNIVERSAL_FACE) != face {
            return Err(fault(
                &format!("{ADD} v"),
                SqlState::SPATIAL_NODES_IN_DIFFERENT_FACES,
                &format!("nodes {start} and {end} are in different faces"),
            ));
        }
        if !self.within_face(&line, face)? {
            return Err(fault(
                &format!("{ADD} vi"),
                SqlState::SPATIAL_GEOMETRY_NOT_WITHIN_FACE,
                &format!("the curve is not within face {face}"),
            ));
        }
        self.ends_at(&line, (start, end), &format!("{ADD} vii"))?;
        self.no_isolated_node_on(&line, &format!("{ADD} ix"))?;
        self.meets_no_edge(&line, (None, &[]), (&format!("{ADD} x"), SqlState::SPATIAL_GEOMETRY_INTERSECTS_AN_EDGE))?;
        if start == end {
            return self.insert_edge((start, end), curve, super::insert::Faces::Modify);
        }
        let id = self.new_edge_id()?;
        let back = id.checked_neg().unwrap_or(id);
        self.edges
            .insert(id, Edge { start, end, next_left: back, next_right: id, left_face: face, right_face: face, geometry: curve.clone() });
        self.connect(&[start, end]);
        self.adopt_srid(curve);
        Ok(id)
    }

    /// [§11.2.2 Description 5] The nodes `nodes`, now ends of edges, of no
    /// containing face.
    pub(super) fn connect(&mut self, nodes: &[i32]) {
        for id in nodes {
            if let Some(n) = self.nodes.get_mut(id) {
                n.containing_face = None;
            }
        }
    }

    /// [§11.2.2 Description 5] The nodes of `nodes` no edge ends at any
    /// more, each in face `face`.
    pub(super) fn isolate(&mut self, nodes: &[i32], face: i32) {
        for id in nodes {
            if self.isolated(*id)
                && let Some(n) = self.nodes.get_mut(id)
            {
                n.containing_face = Some(face);
            }
        }
    }

    /// [§11.3.7] ST_RemIsoEdge: isolated edge `edge` removed. Reading: Table
    /// 16 has no "not isolated edge", so its three cases raise "not isolated
    /// node", as an edge is isolated when its nodes are; an edge that is
    /// not there is SQLSTATE 2FF43. Its nodes are left isolated in its face.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF43 (non-existent edge); 2FF50 if its faces differ or
    /// another edge ends at one of its nodes.
    pub fn remove_iso_edge(&mut self, edge: i32) -> Result<()> {
        let e = self.edge(edge, REMOVE)?.clone();
        if e.left_face != e.right_face {
            return Err(fault(
                &format!("{REMOVE} i"),
                SqlState::SPATIAL_NOT_ISOLATED_NODE,
                &format!("edge {edge} is not isolated: its faces differ"),
            ));
        }
        for (node, at) in [(e.start, "ii"), (e.end, "iii")] {
            if self.has_edges(node, &[edge]) {
                return Err(fault(
                    &format!("{REMOVE} {at}"),
                    SqlState::SPATIAL_NOT_ISOLATED_NODE,
                    &format!("edge {edge} is not isolated: another edge ends at node {node}"),
                ));
            }
        }
        self.edges.remove(&edge);
        self.isolate(&[e.start, e.end], e.left_face);
        Ok(())
    }

    /// [§11.3.6] ST_ChangeEdgeGeom: edge `edge` given the geometry `curve`,
    /// its next edges at its nodes set again from it. Reading: an edge that
    /// is not there is SQLSTATE 2FF43; the text's "anothernode" of
    /// Description 2g ii is the edge's end node.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF29 (curve not simple); 2FF43; 2FF57, 2FF33; 2FF34
    /// (geometry crosses a node); 2FF36 (geometry intersects an edge) if
    /// it meets another edge but at its nodes; 2FF10.
    pub fn change_edge_geom(&mut self, edge: i32, curve: &Geom) -> Result<()> {
        self.same_srid(curve, "§11.3.6 Description 3")?;
        let line = simple_line(curve, "§11.3.6 Description 2f")?;
        let e = self.edge(edge, CHANGE)?.clone();
        let (p1, p2) = self.ends_at(&line, (e.start, e.end), &format!("{CHANGE} i"))?;
        self.no_isolated_node_on(&line, &format!("{CHANGE} iii"))?;
        self.meets_no_edge(&line, (Some(edge), &[p1, p2]), (&format!("{CHANGE} iv"), SqlState::SPATIAL_GEOMETRY_INTERSECTS_AN_EDGE))?;
        if let Some(row) = self.edges.get_mut(&edge) {
            row.geometry = curve.clone();
        }
        self.relink(&[e.start, e.end])?;
        self.refresh_mbr(&[e.left_face, e.right_face])
    }
}
