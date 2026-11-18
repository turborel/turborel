//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.3, §11] Topology-geometry:
//! the winged-edge model of a planar graph of nodes, edges and faces, and
//! the routines of §11.3 that edit it, as pure operations on a topology
//! held in memory. The SQL layer reads the rows of ST_TOPO_GEO.ST_NODE,
//! ST_EDGE and ST_FACE of one topology into a [`Topology`], runs a
//! routine, and writes back the rows that changed (`Topology::changes`).
//!
//! Every edge carries its next left and next right edge [§11.2.3
//! Description 6, 7], signed by the direction it is walked in. A routine
//! that adds, removes or reshapes an edge recomputes them at the nodes it
//! touched from the cyclic order of the edges there (`star.rs`), then
//! traces the rings those pointers make to split or merge faces
//! (`rings.rs`). Point location, crossing and ring orientation are decided
//! by exact predicates (`predicates.rs`), so no tolerance enters a
//! decision.

mod create;
mod edges;
mod heal;
mod insert;
mod locate;
mod nodes;
mod noding;
mod predicates;
mod remove;
mod rings;
mod split;
pub mod sql;
mod star;
mod validate;

use std::collections::BTreeMap;

pub use create::create;
pub use insert::Faces;
pub use rings::{face_edges, face_geometry};
pub use validate::{Problem, validate};

use crate::error::{Code, Result, SqlError, SqlState};
use crate::geometry::model::{Body, Coords, Curve, Dims, Geom};

/// A position in x and y.
pub type Point = [f64; 2];

/// [§11.1.1] The identifier of the universal face, which every topology
/// has and which has no geometry.
pub const UNIVERSAL_FACE: i32 = 0;

/// [§11.2.2] A node: its location and, of an isolated node, the face it
/// lies in.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// GEOMETRY, an ST_Point.
    pub geometry: Geom,
    /// CONTAINING_FACE, none unless the node is isolated [Description 5].
    pub containing_face: Option<i32>,
}

/// [§11.2.3] An edge of the winged-edge model.
#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    /// START_NODE.
    pub start: i32,
    /// END_NODE.
    pub end: i32,
    /// NEXT_LEFT_EDGE, signed: the edge after this one round its left
    /// face, negative if it is walked from its end to its start.
    pub next_left: i32,
    /// NEXT_RIGHT_EDGE, signed as `next_left`, round its right face.
    pub next_right: i32,
    /// LEFT_FACE.
    pub left_face: i32,
    /// RIGHT_FACE.
    pub right_face: i32,
    /// GEOMETRY, an ST_Curve from the start node to the end node.
    pub geometry: Geom,
}

/// [§11.2.4] A face: its minimum bounding rectangle, none for the
/// universal face.
#[derive(Debug, Clone, PartialEq)]
pub struct Face {
    /// MBR, an ST_Polygon.
    pub mbr: Option<Geom>,
}

/// [§11.1.1] One topology: every node, edge and face of it by identifier.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Topology {
    /// The spatial reference system of every geometry of it [§11.2.2
    /// Description 6]: that of the first geometry read or added, none
    /// before there is one.
    pub srid: Option<i32>,
    /// ST_NODE.
    pub nodes: BTreeMap<i32, Node>,
    /// ST_EDGE.
    pub edges: BTreeMap<i32, Edge>,
    /// ST_FACE.
    pub faces: BTreeMap<i32, Face>,
}

/// A row of one of the three tables that a routine changed.
#[derive(Debug, Clone, PartialEq)]
pub enum Change<T> {
    /// Inserted with this identifier.
    Insert(i32, T),
    /// Updated to this.
    Update(i32, T),
    /// Deleted.
    Delete(i32),
}

/// The rows of the three tables a routine changed, deletions first.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Changes {
    /// Of ST_NODE.
    pub nodes: Vec<Change<Node>>,
    /// Of ST_EDGE.
    pub edges: Vec<Change<Edge>>,
    /// Of ST_FACE.
    pub faces: Vec<Change<Face>>,
}

/// The changes that make `before` into `after`.
fn diff<T: Clone + PartialEq>(before: &BTreeMap<i32, T>, after: &BTreeMap<i32, T>) -> Vec<Change<T>> {
    let deleted = before.keys().filter(|id| !after.contains_key(id)).map(|id| Change::Delete(*id));
    let rest = after.iter().filter_map(|(id, row)| match before.get(id) {
        None => Some(Change::Insert(*id, row.clone())),
        Some(old) => (old != row).then(|| Change::Update(*id, row.clone())),
    });
    deleted.chain(rest).collect()
}

impl Topology {
    /// The rows that differ between `self`, as it was read, and `after`.
    pub fn changes(&self, after: &Topology) -> Changes {
        Changes { nodes: diff(&self.nodes, &after.nodes), edges: diff(&self.edges, &after.edges), faces: diff(&self.faces, &after.faces) }
    }

    /// The node `id`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF46 (non-existent node) if there is none, of the rule
    /// `rule`.
    pub fn node(&self, id: i32, rule: &str) -> Result<&Node> {
        self.nodes.get(&id).ok_or_else(|| fault(rule, SqlState::SPATIAL_NON_EXISTENT_NODE, &format!("no node {id}")))
    }

    /// The edge `id`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF43 (non-existent edge) if there is none, of `rule`.
    pub fn edge(&self, id: i32, rule: &str) -> Result<&Edge> {
        self.edges.get(&id).ok_or_else(|| fault(rule, SqlState::SPATIAL_NON_EXISTENT_EDGE, &format!("no edge {id}")))
    }

    /// The spatial reference system of its geometries, 0 if it has none.
    pub fn srid_or_unknown(&self) -> i32 {
        self.srid.unwrap_or_default()
    }

    /// [§11.3.1 Description 3] That `g` is of the topology's system, if it
    /// has one.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF10 (mixed spatial reference systems) if it is not.
    pub fn same_srid(&self, g: &Geom, rule: &str) -> Result<()> {
        if let Some(s) = self.srid
            && s != g.srid
        {
            return Err(fault(
                rule,
                SqlState::SPATIAL_MIXED_SPATIAL_REFERENCE_SYSTEMS,
                &format!("the topology's system is {s}, not {}", g.srid),
            ));
        }
        Ok(())
    }

    /// The system of `g` the topology's, if it had none.
    fn adopt_srid(&mut self, g: &Geom) {
        self.srid.get_or_insert(g.srid);
    }

    /// The location of node `id`, in x and y.
    fn at(&self, id: i32) -> Result<Point> {
        let node = self.nodes.get(&id).ok_or_else(|| damaged(&format!("an edge ends at node {id}, which is not there")))?;
        point_of(&node.geometry)
    }

    /// Whether node `id` is the start or end of any edge but those of
    /// `except`.
    fn has_edges(&self, id: i32, except: &[i32]) -> bool {
        self.edges.iter().any(|(e, edge)| !except.contains(e) && (edge.start == id || edge.end == id))
    }

    /// A node identifier no node has [§11.3.1 Description 2e iv 1].
    fn new_node_id(&self) -> Result<i32> {
        next_id(self.nodes.keys().next_back())
    }

    /// An edge identifier no edge has.
    fn new_edge_id(&self) -> Result<i32> {
        next_id(self.edges.keys().next_back())
    }

    /// A face identifier no face has, never the universal face's.
    fn new_face_id(&self) -> Result<i32> {
        next_id(self.faces.keys().next_back())
    }
}

/// The identifier after the greatest of those in use, 1 of none.
fn next_id(last: Option<&i32>) -> Result<i32> {
    let last = last.copied().unwrap_or(UNIVERSAL_FACE).max(UNIVERSAL_FACE);
    last.checked_add(1).ok_or_else(|| SqlError::new(Code::Unsupported, "[DESIGN] a topology has no identifier left for a new primitive"))
}

/// An exception of SQL/MM Spatial: `state` of the rule `rule`.
pub(crate) fn fault(rule: &str, state: SqlState, detail: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) {rule}] {detail}")).with_state(state)
}

/// SQLCODE -916: the rows of a topology contradict themselves in a way no
/// routine of §11.3 can work on.
pub(crate) fn damaged(detail: &str) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[DESIGN] the topology is inconsistent: {detail}"))
}

/// The x and y of point `g`.
///
/// # Errors
///
/// SQLSTATE 2FF17 (empty point value) of an empty point; 2FF02 of a value
/// that is no point.
pub fn point_of(g: &Geom) -> Result<Point> {
    let Body::Point(c) = &g.body else {
        return Err(fault("§11.2.2 Description 4", SqlState::SPATIAL_INVALID_ARGUMENT, "a node's geometry is no point"));
    };
    match (c.first(), c.get(1)) {
        (Some(x), Some(y)) => Ok([*x, *y]),
        (None, _) | (_, None) => Err(fault("§11.2.2 Description 4", SqlState::SPATIAL_EMPTY_POINT_VALUE, "a node's point is empty")),
    }
}

/// The x and y of each point of the linear form of curve `g`.
///
/// # Errors
///
/// SQLSTATE 2FF02 of a value that is no curve or an empty one; SQLCODE
/// -900 of a curve no linearizer takes.
pub fn line_of(g: &Geom) -> Result<Vec<Point>> {
    let Body::Curve(Curve::LineString(c)) = &g.linear()?.body else {
        return Err(fault("§11.2.3 Description 10", SqlState::SPATIAL_INVALID_ARGUMENT, "an edge's geometry is no curve"));
    };
    let points = xy_of(c, g.dims);
    if points.len() < 2 {
        return Err(fault("§11.2.3 Description 10", SqlState::SPATIAL_INVALID_ARGUMENT, "an edge's curve is empty"));
    }
    Ok(points)
}

/// The x and y of each point of `c`, of dimensions `dims`.
fn xy_of(c: &Coords, dims: Dims) -> Vec<Point> {
    c.chunks_exact(dims.stride()).filter_map(|p| Some([*p.first()?, *p.get(1)?])).collect()
}

/// A linestring of `points`, with z and m of `dims` (zero where a point
/// has none), of `srid`.
///
/// # Errors
///
/// As `Geom::new`.
fn linestring(points: Vec<f64>, (srid, dims): (i32, Dims)) -> Result<Geom> {
    Geom::new(srid, dims, Body::Curve(Curve::LineString(points)))
}

#[cfg(test)]
mod tests;
