//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3.8 to §12.3.11] A link
//! split at a new node: of a logical network, ST_NewLogLinkSplit and
//! ST_ModLogLinkSplit; of a spatial one, ST_NewGeoLinkSplit and
//! ST_ModGeoLinkSplit, which cut the link's curve at the node's point.
//! Each makes either two new links in place of the one split, or keeps
//! the link split as the first part and adds one, all in the direction of
//! the link split.

use super::points::{coords_of, cut, linestring, point_xy, within};
use super::{Link, Network, Replacing, damaged, fault};
use crate::error::{Result, SqlState};
use crate::geometry::model::Geom;

/// How a split makes its links.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Split {
    /// Two new links in place of the one split (ST_NewLogLinkSplit,
    /// ST_NewGeoLinkSplit).
    New,
    /// The link split kept as the first part, one new link the second
    /// (ST_ModLogLinkSplit, ST_ModGeoLinkSplit).
    Modify,
}

impl Network {
    /// The new node `node`, of geometry `point`, put between the ends of
    /// link `link`, `old`, as `how` says, with the parts' geometries
    /// `parts`.
    fn put(&mut self, (link, old): (i32, Link), node: (i32, Option<Geom>), how: Split, parts: (Option<Geom>, Option<Geom>)) -> Result<()> {
        let (id, point) = node;
        self.nodes.insert(id, point);
        let first = Link { start: old.start, end: id, geometry: parts.0 };
        let second = Link { start: id, end: old.end, geometry: parts.1 };
        // [§12.3.8 Description 2e iii 5] New identifiers: the first is
        // found before the link split is removed, so neither is its.
        let id1 = self.new_link_id()?;
        match how {
            Split::New => {
                self.links.remove(&link);
                let id2 = id1.checked_add(1).ok_or_else(|| damaged("no link identifier is left"))?;
                self.links.insert(id1, first);
                self.links.insert(id2, second);
            }
            Split::Modify => {
                self.links.insert(link, first);
                self.links.insert(id1, second);
            }
        }
        Ok(())
    }

    /// [§12.3.8, §12.3.9] Logical link `link` split by a new node of no
    /// geometry, as `how` says; the new node's identifier.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF45 (non-existent link); 2FF49 (not a logical link) if
    /// the link has geometry.
    pub fn split_logical(&mut self, link: i32, how: Split) -> Result<i32> {
        let rule = match how {
            Split::New => "§12.3.8 Description 2e",
            Split::Modify => "§12.3.9 Description 2e",
        };
        let old = self.link(link, &format!("{rule} i"))?.clone();
        if old.geometry.is_some() {
            return Err(fault(&format!("{rule} ii"), SqlState::SPATIAL_NOT_A_LOGICAL_LINK, &format!("link {link} has geometry")));
        }
        let node = self.new_node_id()?;
        self.put((link, old), (node, None), how, (None, None))?;
        Ok(node)
    }

    /// [§12.3.10, §12.3.11] Link `link` split by a new node at `point`, its
    /// curve cut there, as `how` says; the new node's identifier.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF45 (non-existent link); 2FF38 (link has null geometry);
    /// 2FF55 (point not on link) if the point is not within the link's
    /// curve; 2FF10.
    pub fn split_spatial(&mut self, link: i32, point: &Geom, how: Split) -> Result<i32> {
        let (rule, systems) = match how {
            Split::New => ("§12.3.10 Description 2f", "§12.3.10 Description 3"),
            Split::Modify => ("§12.3.11 Description 2f", "§12.3.11 Description 3"),
        };
        let old = self.link(link, &format!("{rule} i"))?.clone();
        let g = old.geometry.clone().ok_or_else(|| {
            fault(&format!("{rule} ii"), SqlState::SPATIAL_LINK_HAS_NULL_GEOMETRY, &format!("link {link} has no geometry"))
        })?;
        let coords = coords_of(&g)?;
        let at = point_xy(point).and_then(|p| within(p, &coords, g.dims));
        let k = at.ok_or_else(|| {
            fault(&format!("{rule} iii"), SqlState::SPATIAL_POINT_NOT_ON_LINK, &format!("the point is not within link {link}"))
        })?;
        self.same_srid(Some(point), Replacing::Nothing, systems)?;
        let (first, second) = cut(&coords, g.dims, k, point);
        let parts = (Some(linestring(first, &g)?), Some(linestring(second, &g)?));
        let node = self.new_node_id()?;
        self.put((link, old), (node, Some(point.clone())), how, parts)?;
        Ok(node)
    }
}
