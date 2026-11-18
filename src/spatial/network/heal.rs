//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3.12, §12.3.13]
//! ST_NewLinkHeal and ST_ModLinkHeal: two links joined at a node no other
//! link ends at, that node removed, into one link in the direction of the
//! first: a new link, or the first link itself.

use super::points::{coords_of, joined, linestring, reversed};
use super::{Link, Network, damaged, fault};
use crate::error::{Result, SqlState};
use crate::geometry::model::Geom;

/// How a heal makes its link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Heal {
    /// A new link in place of both (ST_NewLinkHeal).
    New,
    /// The first link kept, made the whole (ST_ModLinkHeal).
    Modify,
}

/// [Description 2f iii 5] How the two links meet, CASE 1 to 4: the
/// common node is the first's end and the second's start, both ends, both
/// starts, or the first's start and the second's end.
#[derive(Debug, Clone, Copy)]
enum Meet {
    EndStart,
    EndEnd,
    StartStart,
    StartEnd,
}

/// How links `l1` and `l2` meet, and at which node, tested in the order
/// the text gives; none if they do not.
fn meet(l1: &Link, l2: &Link) -> Option<(Meet, i32)> {
    if l1.end == l2.start {
        Some((Meet::EndStart, l1.end))
    } else if l1.end == l2.end {
        Some((Meet::EndEnd, l1.end))
    } else if l1.start == l2.start {
        Some((Meet::StartStart, l1.start))
    } else if l1.start == l2.end {
        Some((Meet::StartEnd, l1.start))
    } else {
        None
    }
}

/// [Description 2f v 7] The start and end nodes of the link healed of
/// `l1` and `l2`, which meet as `how`.
fn healed_ends(how: Meet, l1: &Link, l2: &Link) -> (i32, i32) {
    match how {
        Meet::EndStart => (l1.start, l2.end),
        Meet::EndEnd => (l1.start, l2.start),
        Meet::StartStart => (l2.end, l1.end),
        Meet::StartEnd => (l2.start, l1.end),
    }
}

/// [Description 2f v 7 I] The curve of the link healed of curves `g1` and
/// `g2`, which meet as `how`: none if either is none.
///
/// # Errors
///
/// SQLSTATE 2FF25 (mixed coordinate dimensions) if the two curves'
/// dimensions differ; the linearizer's.
fn healed_curve(how: Meet, g1: Option<&Geom>, g2: Option<&Geom>, rule: &str) -> Result<Option<Geom>> {
    let (Some(g1), Some(g2)) = (g1, g2) else { return Ok(None) };
    let dims = g1.dims;
    if g2.dims != dims {
        return Err(fault(rule, SqlState::SPATIAL_MIXED_COORDINATE_DIMENSIONS, "the two curves have different coordinate dimensions"));
    }
    let (c1, c2) = (coords_of(g1)?, coords_of(g2)?);
    let coords = match how {
        Meet::EndStart => joined(c1, &c2, dims),
        Meet::EndEnd => joined(c1, &reversed(&c2, dims), dims),
        Meet::StartStart => joined(reversed(&c2, dims), &c1, dims),
        Meet::StartEnd => joined(c2, &c1, dims),
    };
    linestring(coords, g1).map(Some)
}

impl Network {
    /// [§12.3.12, §12.3.13] Links `first` and `second` healed into one, as
    /// `how` says: the new link's identifier, or none if the first is kept.
    /// Reading: a link healed with itself is SQLSTATE 2FF41, as it is no
    /// pair of links to connect.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF45 (non-existent link); 2FF41 (non-connected links);
    /// 2FF53 (other links connected); 2FF25.
    pub fn heal(&mut self, (first, second): (i32, i32), how: Heal) -> Result<Option<i32>> {
        let rule = match how {
            Heal::New => "§12.3.12 Description 2f",
            Heal::Modify => "§12.3.13 Description 2f",
        };
        let l1 = self.link(first, &format!("{rule} i"))?.clone();
        let l2 = self.link(second, &format!("{rule} ii"))?.clone();
        let found = if first == second { None } else { meet(&l1, &l2) };
        let (meets, common) = found.ok_or_else(|| {
            fault(&format!("{rule} iii 5E"), SqlState::SPATIAL_NON_CONNECTED_LINKS, &format!("links {first} and {second} share no node"))
        })?;
        if self.has_links(common, &[first, second]) {
            return Err(fault(
                &format!("{rule} iv"),
                SqlState::SPATIAL_OTHER_LINKS_CONNECTED,
                &format!("another link ends at node {common}"),
            ));
        }
        let geometry = healed_curve(meets, l1.geometry.as_ref(), l2.geometry.as_ref(), &format!("{rule} v 7"))?;
        let (start, end) = healed_ends(meets, &l1, &l2);
        self.nodes.remove(&common);
        let made = self.new_link_id()?;
        self.links.remove(&second);
        let healed = Link { start, end, geometry };
        match how {
            Heal::New => {
                self.links.remove(&first);
                self.links.insert(made, healed);
                Ok(Some(made))
            }
            Heal::Modify => {
                let kept = self.links.get_mut(&first).ok_or_else(|| damaged("the first link is gone"))?;
                *kept = healed;
                Ok(None)
            }
        }
    }
}
