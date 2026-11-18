//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3.17, §12.3.18]
//! ST_ValidLogicalNet and ST_ValidSpatialNet: the inconsistencies of a
//! logical or a spatial network, each a row of an error and the one or two
//! primitives it is of, in the order the Description lists the checks,
//! each check's rows by identifier.

use std::collections::BTreeSet;

use super::points::{ends, equal};
use super::{Link, Network, fault};
use crate::error::{Result, SqlState};
use crate::geometry::model::Geom;

/// [§12.3.18 Description 2d ii 2A] The error of a node of no geometry.
pub const MISSING_NODE_GEOMETRY: &str = "missing node geometry";
/// [Description 2d ii 3A] The error of a link of no geometry.
pub const MISSING_LINK_GEOMETRY: &str = "missing link geometry";
/// [Description 2d ii 4A] The error of a curve that does not start at its
/// start node.
pub const START_MISMATCH: &str = "geometry start mismatch";
/// [Description 2d ii 5A] The error of a curve that does not end at its
/// end node.
pub const END_MISMATCH: &str = "geometry end mismatch";
/// [Description 2d ii 6A] The error of geometries of several systems.
pub const MIXED_SRIDS: &str = "mixed SRIDs";
/// [§12.3.17 Description 2d ii 2A] The error of a node of a logical
/// network that has geometry.
pub const NODE_HAS_GEOMETRY: &str = "node has geometry";
/// [§12.3.17 Description 2d ii 3A] The error of a link of a logical
/// network that has geometry.
pub const LINK_HAS_GEOMETRY: &str = "link has geometry";

/// A row of the table a validation returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// ERROR.
    pub error: &'static str,
    /// PRIMITIVE1: the first primitive's identifier.
    pub primitive1: Option<i32>,
    /// PRIMITIVE2: the second's.
    pub primitive2: Option<i32>,
}

impl Problem {
    /// A problem `error` of `primitive1` and `primitive2`.
    fn of(error: &'static str, primitive1: Option<i32>, primitive2: Option<i32>) -> Problem {
        Problem { error, primitive1, primitive2 }
    }
}

/// [Description 2d i] That `net` is not empty.
///
/// # Errors
///
/// SQLSTATE 2FF31 (empty network) of `rule` if it has neither node nor
/// link.
fn not_empty(net: &Network, rule: &str) -> Result<()> {
    if net.is_empty() {
        return Err(fault(rule, SqlState::SPATIAL_EMPTY_NETWORK, "the network has neither node nor link"));
    }
    Ok(())
}

/// [§12.3.17] The inconsistencies of `net` as a logical network: each
/// node, then each link, that has geometry.
///
/// # Errors
///
/// SQLSTATE 2FF31 (empty network).
pub fn logical_problems(net: &Network) -> Result<Vec<Problem>> {
    not_empty(net, "§12.3.17 Description 2d i")?;
    let nodes = net.nodes.iter().filter(|(_, g)| g.is_some()).map(|(id, _)| Problem::of(NODE_HAS_GEOMETRY, Some(*id), None));
    let links = net.links.iter().filter(|(_, l)| l.geometry.is_some()).map(|(id, _)| Problem::of(LINK_HAS_GEOMETRY, Some(*id), None));
    Ok(nodes.chain(links).collect())
}

/// [§12.3.18 Description 2d ii 4, 5] Whether the end of curve `curve` at
/// `at_start` or not is the point `node`. A node of no geometry, or a
/// curve of no end, is not: ST_Equals of the null value is not 1.
fn ends_at(curve: &Geom, node: Option<&Geom>, at_start: bool) -> Result<bool> {
    let (Some(node), Some((s, e))) = (node, ends(curve)?) else { return Ok(false) };
    equal(node, if at_start { &s } else { &e })
}

/// [Description 2d ii 4, 5] The mismatches of the ends of each link of
/// `net` that has geometry: those of the start, then those of the end.
fn mismatches(net: &Network) -> Result<Vec<Problem>> {
    let mut starts = Vec::new();
    let mut finishes = Vec::new();
    let geometry = |id: i32| net.nodes.get(&id).and_then(Option::as_ref);
    for (id, Link { start, end, geometry: g }) in &net.links {
        let Some(g) = g else { continue };
        if !ends_at(g, geometry(*start), true)? {
            starts.push(Problem::of(START_MISMATCH, Some(*id), Some(*start)));
        }
        if !ends_at(g, geometry(*end), false)? {
            finishes.push(Problem::of(END_MISMATCH, Some(*id), Some(*end)));
        }
    }
    starts.extend(finishes);
    Ok(starts)
}

/// [§12.3.18] The inconsistencies of `net` as a spatial network: nodes
/// and links of no geometry, curves whose ends are not their nodes', and
/// one row if its geometries are of more than one system. Reading: the
/// last is found among all its geometries, as the heading of Description
/// 2d ii 6 says, not only against a node's.
///
/// # Errors
///
/// SQLSTATE 2FF31 (empty network); a curve's that is not well formed.
pub fn spatial_problems(net: &Network) -> Result<Vec<Problem>> {
    not_empty(net, "§12.3.18 Description 2d i")?;
    let nodes = net.nodes.iter().filter(|(_, g)| g.is_none()).map(|(id, _)| Problem::of(MISSING_NODE_GEOMETRY, Some(*id), None));
    let links = net.links.iter().filter(|(_, l)| l.geometry.is_none()).map(|(id, _)| Problem::of(MISSING_LINK_GEOMETRY, Some(*id), None));
    let mut out: Vec<Problem> = nodes.chain(links).collect();
    out.extend(mismatches(net)?);
    let node_systems = net.nodes.values().flatten().map(|g| g.srid);
    let systems: BTreeSet<i32> = node_systems.chain(net.links.values().filter_map(|l| l.geometry.as_ref()).map(|g| g.srid)).collect();
    if systems.len() > 1 {
        out.push(Problem::of(MIXED_SRIDS, None, None));
    }
    Ok(out)
}
