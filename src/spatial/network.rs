//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12] Topology-networks: a
//! graph of nodes and directed links, logical (no geometry) or spatial
//! (points and curves), and the routines of §12.3 that edit and check it,
//! as pure operations on a network held in memory. The SQL layer
//! (`network/sql.rs`) reads the rows of one network's views into a
//! [`Network`], runs a routine, and writes back the rows that changed
//! (`Network::changes`), as the topology-geometry routines of §11 do.
//!
//! A network keeps no planar structure: links may cross without a node,
//! as a road bridges another, so a routine checks only what its
//! Description names. Where it compares a node with a curve's end, it
//! compares them as ST_Equals does (`geometry::EQUALITY`), and a point on
//! a link is decided by exact orientation (`points.rs`), so no tolerance
//! enters a decision.

mod build;
mod edit;
mod heal;
mod points;
mod split;
pub mod sql;
mod validate;

use std::collections::BTreeMap;

pub use validate::{Problem, logical_problems, spatial_problems};

use crate::error::{Code, Result, SqlError, SqlState};
use crate::geometry::model::Geom;

/// [§12.2.3] A link: the node at its start, the node at its end, and its
/// geometry, an ST_Curve from the one to the other, none in a logical
/// network [Description 6].
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    /// START_NODE.
    pub start: i32,
    /// END_NODE.
    pub end: i32,
    /// GEOMETRY.
    pub geometry: Option<Geom>,
}

/// [§12.1.1] One network: each node's geometry, an ST_Point or none
/// [§12.2.2 Description 4], and each link, by identifier.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Network {
    /// ST_NODE.
    pub nodes: BTreeMap<i32, Option<Geom>>,
    /// ST_LINK.
    pub links: BTreeMap<i32, Link>,
}

/// A row of one of the two tables that a routine changed.
#[derive(Debug, Clone, PartialEq)]
pub enum Change<T> {
    /// Inserted with this identifier.
    Insert(i32, T),
    /// Updated to this.
    Update(i32, T),
    /// Deleted.
    Delete(i32),
}

/// The rows of the two tables a routine changed.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Changes {
    /// Of ST_NODE.
    pub nodes: Vec<Change<Option<Geom>>>,
    /// Of ST_LINK.
    pub links: Vec<Change<Link>>,
}

/// Which geometry of a network a routine replaces, so its own old value
/// does not decide the network's spatial reference system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Replacing {
    /// None: the geometry is added.
    Nothing,
    /// That of this node.
    Node(i32),
    /// That of this link.
    Link(i32),
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

impl Network {
    /// The rows that differ between `self`, as it was read, and `after`.
    pub fn changes(&self, after: &Network) -> Changes {
        Changes { nodes: diff(&self.nodes, &after.nodes), links: diff(&self.links, &after.links) }
    }

    /// Whether it has neither node nor link.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.links.is_empty()
    }

    /// The link `id`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF45 (non-existent link) if there is none, of the rule
    /// `rule`.
    pub fn link(&self, id: i32, rule: &str) -> Result<&Link> {
        self.links.get(&id).ok_or_else(|| fault(rule, SqlState::SPATIAL_NON_EXISTENT_LINK, &format!("no link {id}")))
    }

    /// Whether node `id` is the start or end of any link but those of
    /// `except`.
    pub fn has_links(&self, id: i32, except: &[i32]) -> bool {
        self.links.iter().any(|(l, link)| !except.contains(l) && (link.start == id || link.end == id))
    }

    /// [§12.3.1 Description 2d i] A node identifier no node has.
    fn new_node_id(&self) -> Result<i32> {
        next_id(self.nodes.keys().next_back())
    }

    /// A link identifier no link has.
    fn new_link_id(&self) -> Result<i32> {
        next_id(self.links.keys().next_back())
    }

    /// The spatial reference system of the network's geometries but the
    /// one `replacing` names, none if it has no other.
    fn srid(&self, replacing: Replacing) -> Option<i32> {
        let nodes = self.nodes.iter().filter(|(id, _)| replacing != Replacing::Node(**id)).filter_map(|(_, g)| g.as_ref());
        let links = self.links.iter().filter(|(id, _)| replacing != Replacing::Link(**id)).filter_map(|(_, l)| l.geometry.as_ref());
        nodes.chain(links).map(|g| g.srid).next()
    }

    /// [§12.2.2 Description 5, §12.3.1 Description 3] That `g`, if given,
    /// is of the network's spatial reference system, if it has one, the
    /// geometry `replacing` names left out.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF10 (mixed spatial reference systems) if it is not.
    fn same_srid(&self, g: Option<&Geom>, replacing: Replacing, rule: &str) -> Result<()> {
        let (Some(g), Some(s)) = (g, self.srid(replacing)) else { return Ok(()) };
        if s == g.srid {
            return Ok(());
        }
        Err(fault(rule, SqlState::SPATIAL_MIXED_SPATIAL_REFERENCE_SYSTEMS, &format!("the network's system is {s}, not {}", g.srid)))
    }
}

/// The identifier after the greatest of those in use, 1 of none.
fn next_id(last: Option<&i32>) -> Result<i32> {
    let last = last.copied().unwrap_or(0_i32).max(0_i32);
    last.checked_add(1).ok_or_else(|| SqlError::new(Code::Unsupported, "[DESIGN] a network has no identifier left for a new primitive"))
}

/// An exception of SQL/MM Spatial: `state` of the rule `rule`.
pub(crate) fn fault(rule: &str, state: SqlState, detail: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) {rule}] {detail}")).with_state(state)
}

/// SQLCODE -916: the rows of a network contradict themselves in a way no
/// routine of §12.3 can work on.
pub(crate) fn damaged(detail: &str) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[DESIGN] the network is inconsistent: {detail}"))
}

#[cfg(test)]
mod tests;
