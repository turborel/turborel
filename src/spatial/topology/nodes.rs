//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.1 to §11.3.3] The
//! routines of isolated nodes: ST_AddIsoNode, ST_MoveIsoNode and
//! ST_RemIsoNode, each checking in the order its Description gives.

use super::{Node, Topology, fault, point_of};
use crate::error::{Result, SqlState};
use crate::geometry::model::Geom;

/// [§11.3.1] The rule of ST_AddIsoNode's checks.
const ADD: &str = "§11.3.1 Description 2e";
/// [§11.3.2] The rule of ST_MoveIsoNode's checks.
const MOVE: &str = "§11.3.2 Description 2f";
/// [§11.3.3] The rule of ST_RemIsoNode's checks.
const REMOVE: &str = "§11.3.3 Description 2e";

impl Topology {
    /// [§11.3.1] ST_AddIsoNode: a new isolated node at `point` in face
    /// `face`, or in the face it is within if `face` is none; its
    /// identifier.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF28 (coincident node) if a node is there; 2FF30 (edge
    /// crosses node) if an edge passes through it; 2FF51 (not within face)
    /// if it is not within `face`; 2FF10 (mixed spatial reference systems)
    /// of a point of another system [Description 3].
    pub fn add_iso_node(&mut self, face: Option<i32>, point: &Geom) -> Result<i32> {
        self.same_srid(point, "§11.3.1 Description 3")?;
        let p = point_of(point)?;
        self.no_coincident_node(p, None, &format!("{ADD} i"))?;
        self.no_edge_through(p, &format!("{ADD} ii"))?;
        let within = self.face_at(p)?;
        let face = face.unwrap_or(within);
        if face != within {
            return Err(fault(
                &format!("{ADD} iii"),
                SqlState::SPATIAL_NOT_WITHIN_FACE,
                &format!("the point is within face {within}, not {face}"),
            ));
        }
        let id = self.new_node_id()?;
        self.nodes.insert(id, Node { geometry: point.clone(), containing_face: Some(face) });
        self.adopt_srid(point);
        Ok(id)
    }

    /// [§11.3.2] ST_MoveIsoNode: isolated node `node` moved to `point`.
    /// Reading: a node that is not there is SQLSTATE 2FF46, as the routines
    /// that name a node raise; the text updates no row of it.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF46 (non-existent node); 2FF28 (coincident node) if a
    /// node is at `point`, the node itself among them; 2FF50 (not isolated
    /// node) if an edge ends at it; 2FF30 (edge crosses node) if an edge
    /// passes through `point`; 2FF10 of a point of another system.
    pub fn move_iso_node(&mut self, node: i32, point: &Geom) -> Result<()> {
        self.same_srid(point, "§11.3.2 Description 3")?;
        self.node(node, MOVE)?;
        let p = point_of(point)?;
        self.no_coincident_node(p, None, &format!("{MOVE} i"))?;
        if !self.isolated(node) {
            return Err(fault(&format!("{MOVE} ii"), SqlState::SPATIAL_NOT_ISOLATED_NODE, &format!("an edge ends at node {node}")));
        }
        self.no_edge_through(p, &format!("{MOVE} iii"))?;
        if let Some(n) = self.nodes.get_mut(&node) {
            n.geometry = point.clone();
        }
        Ok(())
    }

    /// [§11.3.3] ST_RemIsoNode: isolated node `node` removed. Reading: a
    /// node that is not there is SQLSTATE 2FF46.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF46 (non-existent node); 2FF50 (not isolated node) if an
    /// edge ends at it.
    pub fn remove_iso_node(&mut self, node: i32) -> Result<()> {
        self.node(node, REMOVE)?;
        if !self.isolated(node) {
            return Err(fault(&format!("{REMOVE} i"), SqlState::SPATIAL_NOT_ISOLATED_NODE, &format!("an edge ends at node {node}")));
        }
        self.nodes.remove(&node);
        Ok(())
    }
}
