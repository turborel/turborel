//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.19] ST_ValidateTopoGeo:
//! the topological inconsistencies of a topology, one row each, in the
//! order of the text's checks.
//!
//! Readings. Each unordered pair of coincident nodes and of crossing edges
//! is given once, the lesser identifier first, as is each pair of
//! overlapping faces; "face within face" is given of each ordered pair.
//! ST_Crosses of a curve and a point is not defined [§5.1.51], so an edge
//! "crosses" a node it passes through between its ends, and two edges
//! cross where they meet anywhere but at a node both end at. The universal
//! face has no edges when the topology has only isolated nodes, so it is
//! not reported as a face without edges. A face's geometry is the polygon
//! of ST_GetFaceGeometry.

use super::predicates::{inside_line, is_simple, meet_except, same};
use super::rings::polygon;
use super::{Point, Topology, UNIVERSAL_FACE, fault, line_of, point_of};
use crate::error::{Result, SqlState};
use crate::spatial::planar::{self, Geometry};

/// [Description 2d ii 2A] The error of coincident nodes.
pub const COINCIDENT_NODES: &str = "coincident nodes";
/// [Description 2d ii 3A] The error of a node an edge crosses.
pub const EDGE_CROSSED_NODE: &str = "edge crossed node";
/// [Description 2d ii 4A] The error of an edge that is not simple.
pub const EDGE_NOT_SIMPLE: &str = "edge not simple";
/// [Description 2d ii 5A] The error of crossing edges.
pub const EDGE_CROSSES_EDGE: &str = "edge crosses edge";
/// [Description 2d ii 6A, 7A] The error of an edge whose curve does not
/// start or end at its node.
pub const GEOMETRY_MISMATCH: &str = "geometry mismatch";
/// [Description 2d ii 8A] The error of a face no edge bounds.
pub const FACE_WITHOUT_EDGES: &str = "face without edges";
/// [Description 2d ii 9A] The error of overlapping faces.
pub const FACE_OVERLAPS_FACE: &str = "face overlaps face";
/// [Description 2d ii 10A] The error of a face within another.
pub const FACE_WITHIN_FACE: &str = "face within face";
/// [Description 2d ii 11A] The error of a topology with no universal face.
pub const NO_UNIVERSAL_FACE: &str = "no universal face";
/// [Description 2d ii 12A] The error of a face of no valid geometry.
pub const INVALID_FACE_GEOMETRY: &str = "invalid face geometry";
/// [Description 2d ii 13A] The error of geometries of several systems.
pub const MIXED_SRIDS: &str = "mixed SRIDs";

/// A row of the table ST_ValidateTopoGeo returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// The column error.
    pub error: &'static str,
    /// The column primitive1.
    pub primitive1: Option<i32>,
    /// The column primitive2.
    pub primitive2: Option<i32>,
}

/// A problem of `error` between `first` and `second`.
fn problem(error: &'static str, first: Option<i32>, second: Option<i32>) -> Problem {
    Problem { error, primitive1: first, primitive2: second }
}

/// [§11.3.19] ST_ValidateTopoGeo: the inconsistencies of `topo`.
///
/// # Errors
///
/// SQLSTATE 2FF32 (empty topology) if it has no node, no edge and at most
/// one face; SQLCODE -900 of a curve no linearizer takes.
pub fn validate(topo: &Topology) -> Result<Vec<Problem>> {
    if topo.nodes.is_empty() && topo.edges.is_empty() && topo.faces.len() <= 1 {
        return Err(fault("§11.3.19 Description 2d i", SqlState::SPATIAL_EMPTY_TOPOLOGY, "the topology is empty"));
    }
    let nodes: Vec<(i32, Point)> = topo.nodes.iter().map(|(id, n)| Ok((*id, point_of(&n.geometry)?))).collect::<Result<_>>()?;
    let edges: Vec<(i32, Vec<Point>)> = topo.edges.iter().map(|(id, e)| Ok((*id, line_of(&e.geometry)?))).collect::<Result<_>>()?;
    let mut out = Vec::new();
    coincident(&nodes, &mut out);
    crossed_nodes(&nodes, &edges, &mut out);
    out.extend(edges.iter().filter(|(_, l)| !is_simple(l)).map(|(id, _)| problem(EDGE_NOT_SIMPLE, Some(*id), None)));
    crossing_edges(topo, &edges, &mut out)?;
    mismatches(topo, &mut out)?;
    faces(topo, &mut out)?;
    srids(topo, &mut out);
    Ok(out)
}

/// [Description 2d ii 2] Each pair of nodes at one place.
fn coincident(nodes: &[(i32, Point)], out: &mut Vec<Problem>) {
    for (i, (n1, p1)) in nodes.iter().enumerate() {
        for (n2, p2) in nodes.iter().skip(i.saturating_add(1)) {
            if same(*p1, *p2) {
                out.push(problem(COINCIDENT_NODES, Some(*n1), Some(*n2)));
            }
        }
    }
}

/// [Description 2d ii 3] Each node an edge passes through between its ends.
fn crossed_nodes(nodes: &[(i32, Point)], edges: &[(i32, Vec<Point>)], out: &mut Vec<Problem>) {
    for (n, p) in nodes {
        for (e, line) in edges {
            if inside_line(*p, line) {
                out.push(problem(EDGE_CROSSED_NODE, Some(*n), Some(*e)));
            }
        }
    }
}

/// [Description 2d ii 5] Each pair of edges that meet but at a node both
/// end at.
fn crossing_edges(topo: &Topology, edges: &[(i32, Vec<Point>)], out: &mut Vec<Problem>) -> Result<()> {
    for (i, (e1, l1)) in edges.iter().enumerate() {
        for (e2, l2) in edges.iter().skip(i.saturating_add(1)) {
            let shared = shared_nodes(topo, *e1, *e2)?;
            if meet_except(l1, l2, &shared).is_some() {
                out.push(problem(EDGE_CROSSES_EDGE, Some(*e1), Some(*e2)));
            }
        }
    }
    Ok(())
}

/// The places of the nodes edges `e1` and `e2` both end at.
fn shared_nodes(topo: &Topology, e1: i32, e2: i32) -> Result<Vec<Point>> {
    let (a, b) = (topo.edge(e1, "§11.3.19")?, topo.edge(e2, "§11.3.19")?);
    let mut out = Vec::new();
    for n in [a.start, a.end] {
        if (n == b.start || n == b.end)
            && let Some(node) = topo.nodes.get(&n)
        {
            out.push(point_of(&node.geometry)?);
        }
    }
    Ok(out)
}

/// [Description 2d ii 6, 7] Each edge whose curve does not start at its
/// start node, and each whose curve does not end at its end node.
fn mismatches(topo: &Topology, out: &mut Vec<Problem>) -> Result<()> {
    let mut ends = Vec::new();
    for (id, e) in &topo.edges {
        let line = line_of(&e.geometry)?;
        for (node, at) in [(e.start, line.first()), (e.end, line.last())] {
            let fits = match topo.nodes.get(&node) {
                Some(n) => at.is_some_and(|p| point_of(&n.geometry).is_ok_and(|q| same(*p, q))),
                None => false,
            };
            if !fits {
                ends.push((*id, node, at.is_some() && node == e.end));
            }
        }
    }
    // Every start first, then every end, as the text checks them.
    ends.sort_by_key(|(_, _, is_end)| *is_end);
    out.extend(ends.into_iter().map(|(e, n, _)| problem(GEOMETRY_MISMATCH, Some(e), Some(n))));
    Ok(())
}

/// [Description 2d ii 8 to 12] The checks of faces.
fn faces(topo: &Topology, out: &mut Vec<Problem>) -> Result<()> {
    for f in topo.faces.keys().filter(|f| **f != UNIVERSAL_FACE) {
        if !topo.edges.values().any(|e| e.left_face == *f || e.right_face == *f) {
            out.push(problem(FACE_WITHOUT_EDGES, Some(*f), None));
        }
    }
    let srid = topo.srid_or_unknown();
    let shapes = topo.shapes()?;
    let polygons: Vec<(i32, Option<Geometry>)> =
        shapes.iter().map(|(f, s)| (*f, polygon(s, srid).and_then(|g| crate::geometry::engine::engine_of(&g).ok()))).collect();
    for (i, (f1, g1)) in polygons.iter().enumerate() {
        for (f2, g2) in polygons.iter().skip(i.saturating_add(1)) {
            if let (Some(a), Some(b)) = (g1, g2)
                && planar::overlaps(a, b)? == Some(true)
            {
                out.push(problem(FACE_OVERLAPS_FACE, Some(*f1), Some(*f2)));
            }
        }
    }
    for (f1, g1) in &polygons {
        for (f2, g2) in &polygons {
            if let (Some(a), Some(b)) = (g1, g2)
                && f1 != f2
                && planar::within(a, b)? == Some(true)
            {
                out.push(problem(FACE_WITHIN_FACE, Some(*f1), Some(*f2)));
            }
        }
    }
    if !topo.faces.contains_key(&UNIVERSAL_FACE) {
        out.push(problem(NO_UNIVERSAL_FACE, None, None));
    }
    out.extend(polygons.iter().filter(|(_, g)| g.is_none()).map(|(f, _)| problem(INVALID_FACE_GEOMETRY, Some(*f), None)));
    Ok(())
}

/// [Description 2d ii 13] One row if the geometries are of more than one
/// system.
fn srids(topo: &Topology, out: &mut Vec<Problem>) {
    let nodes = topo.nodes.values().map(|n| n.geometry.srid);
    let edges = topo.edges.values().map(|e| e.geometry.srid);
    let faces = topo.faces.values().filter_map(|f| f.mbr.as_ref().map(|m| m.srid));
    let mut all = nodes.chain(edges).chain(faces);
    if let Some(first) = all.next()
        && all.any(|s| s != first)
    {
        out.push(problem(MIXED_SRIDS, None, None));
    }
}
