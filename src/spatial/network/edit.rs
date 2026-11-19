//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3.1 to §12.3.6] The
//! routines that add, move and remove a network's nodes and links, each
//! checking in the order its Description gives.

use super::points::{ends, equal};
use super::{Link, Network, Replacing, fault};
use crate::error::{Result, SqlState};
use crate::geometry::model::Geom;

/// Which end of a link a node is at.
#[derive(Debug, Clone, Copy)]
enum End {
    /// The start: START_NODE and ST_StartPoint.
    Start,
    /// The end: END_NODE and ST_EndPoint.
    End,
}

impl End {
    /// The exception of a curve whose end at this end is not the node's
    /// point: 2FF57 (start node not geometry start point) or 2FF33 (end
    /// node not geometry end point).
    fn mismatch(self) -> SqlState {
        match self {
            End::Start => SqlState::SPATIAL_START_NODE_NOT_GEOMETRY_START,
            End::End => SqlState::SPATIAL_END_NODE_NOT_GEOMETRY_END,
        }
    }

    /// Its name, for a message.
    fn name(self) -> &'static str {
        match self {
            End::Start => "start",
            End::End => "end",
        }
    }
}

/// That the end `end` of curve `curve` is `point`, a node's geometry.
/// Reading: an empty curve has no end, so no node's point is its end.
///
/// # Errors
///
/// SQLSTATE 2FF57 or 2FF33 of rule `rule` if it is not.
fn meets(point: &Geom, curve: &Geom, end: End, rule: &str) -> Result<()> {
    let found = ends(curve)?.map(|(s, e)| match end {
        End::Start => s,
        End::End => e,
    });
    let same = match &found {
        Some(p) => equal(point, p)?,
        None => false,
    };
    if same {
        return Ok(());
    }
    Err(fault(rule, end.mismatch(), &format!("the curve's {} point is not its {} node's point", end.name(), end.name())))
}

impl Network {
    /// [§12.3.1] ST_AddIsoNetNode: a new node of geometry `point`, none in
    /// a logical network; its identifier.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF10 if the point is not of the network's system.
    pub fn add_iso_node(&mut self, point: Option<Geom>) -> Result<i32> {
        self.same_srid(point.as_ref(), Replacing::Nothing, "§12.3.1 Description 3")?;
        let id = self.new_node_id()?;
        self.nodes.insert(id, point);
        Ok(id)
    }

    /// [§12.3.2] ST_MoveIsoNetNode: node `node`, which no link ends at,
    /// given geometry `point`. Reading: a node that is not there is
    /// updated as no row is, so nothing changes.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF50 (not isolated node) if a link ends at it; 2FF10.
    pub fn move_iso_node(&mut self, node: i32, point: Option<Geom>) -> Result<()> {
        if self.has_links(node, &[]) {
            return Err(fault("§12.3.2 Description 2e i", SqlState::SPATIAL_NOT_ISOLATED_NODE, &format!("a link ends at node {node}")));
        }
        self.same_srid(point.as_ref(), Replacing::Node(node), "§12.3.2 Description 3")?;
        if let Some(g) = self.nodes.get_mut(&node) {
            *g = point;
        }
        Ok(())
    }

    /// [§12.3.3] ST_RemIsoNetNode: node `node`, which no link ends at,
    /// removed. Reading: of a node that is not there, no row is deleted.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF50 (not isolated node) if a link ends at it.
    pub fn remove_iso_node(&mut self, node: i32) -> Result<()> {
        if self.has_links(node, &[]) {
            return Err(fault("§12.3.3 Description 2e i", SqlState::SPATIAL_NOT_ISOLATED_NODE, &format!("a link ends at node {node}")));
        }
        self.nodes.remove(&node);
        Ok(())
    }

    /// The geometry of node `node`, which is there.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF46 (non-existent node) of rule `rule` if it is not.
    fn node_geometry(&self, node: i32, rule: &str) -> Result<Option<&Geom>> {
        let found = self.nodes.get(&node).ok_or_else(|| fault(rule, SqlState::SPATIAL_NON_EXISTENT_NODE, &format!("no node {node}")))?;
        Ok(found.as_ref())
    }

    /// [§12.3.4] ST_AddLink: a new link from node `start` to node `end` of
    /// geometry `curve`, none in a logical network; its identifier.
    /// Reading: the link is added once the checks of Description 2f i to
    /// iii pass, of a null curve too.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF46 (non-existent node); 2FF57 or 2FF33 if a node with
    /// geometry is not at the curve's end; 2FF10.
    pub fn add_link(&mut self, (start, end): (i32, i32), curve: Option<Geom>) -> Result<i32> {
        let p1 = self.node_geometry(start, "§12.3.4 Description 2f i")?.cloned();
        let p2 = self.node_geometry(end, "§12.3.4 Description 2f ii")?.cloned();
        if let Some(c) = &curve {
            if let Some(p) = &p1 {
                meets(p, c, End::Start, "§12.3.4 Description 2f iii 1")?;
            }
            if let Some(p) = &p2 {
                meets(p, c, End::End, "§12.3.4 Description 2f iii 2")?;
            }
        }
        self.same_srid(curve.as_ref(), Replacing::Nothing, "§12.3.4 Description 3")?;
        let id = self.new_link_id()?;
        self.links.insert(id, Link { start, end, geometry: curve });
        Ok(id)
    }

    /// The geometry of the node at end `end` of link `link`; none of a
    /// link that is not there, as the text's subquery finds no row.
    fn end_geometry(&self, link: i32, end: End) -> Option<&Geom> {
        let l = self.links.get(&link)?;
        let node = match end {
            End::Start => l.start,
            End::End => l.end,
        };
        self.nodes.get(&node)?.as_ref()
    }

    /// [§12.3.5] ST_ChangeLinkGeom: link `link` given geometry `curve`.
    /// Readings: the node at the link's end is that of `link`, where the
    /// text names `anothernode` [Description 2e i 2]; and the link is
    /// updated once the checks pass, of a null curve too.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF58 (null node geometry) if a node at its end has none,
    /// or the link is not there; 2FF57 or 2FF33 if a node is not at the
    /// curve's end; 2FF10.
    pub fn change_link_geom(&mut self, link: i32, curve: Option<Geom>) -> Result<()> {
        if let Some(c) = &curve {
            for (end, rule) in [(End::Start, "§12.3.5 Description 2e i 1"), (End::End, "§12.3.5 Description 2e i 2")] {
                let p = self.end_geometry(link, end).ok_or_else(|| {
                    fault(
                        &format!("{rule}A"),
                        SqlState::SPATIAL_NULL_NODE_GEOMETRY,
                        &format!("the {} node of link {link} has no geometry", end.name()),
                    )
                })?;
                meets(p, c, end, &format!("{rule}B"))?;
            }
        }
        self.same_srid(curve.as_ref(), Replacing::Link(link), "§12.3.5 Description 3")?;
        if let Some(l) = self.links.get_mut(&link) {
            l.geometry = curve;
        }
        Ok(())
    }

    /// [§12.3.6] ST_RemoveLink: link `link` removed, its nodes kept.
    /// Reading: of a link that is not there, no row is deleted.
    pub fn remove_link(&mut self, link: i32) {
        self.links.remove(&link);
    }
}
