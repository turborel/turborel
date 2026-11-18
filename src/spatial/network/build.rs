//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3.14 to §12.3.16] A
//! network made whole: of a topology's nodes and edges (ST_LogiNetFromTGeo,
//! ST_SpatNetFromTGeo), or of a collection of geometry values
//! (ST_SpatNetFromGeom).
//!
//! Readings of ST_SpatNetFromGeom [§12.3.16 Description 2h iii 1]: each
//! curve of the collection, at any depth, is a link from a node at its
//! start point to a node at its end point, in its direction; each point is
//! a node; a point where nodes are already is that node, as ST_Equals
//! finds two points one; and curves are not cut where they cross, as a
//! network is no planar graph (a road may bridge another). Nodes are
//! numbered from 1 in the order the collection first reaches them, links
//! from 1 in the order of their curves. An empty element is left out. A
//! surface or solid has no network primitive of its own, so it is
//! SQLSTATE 2FF02 (invalid argument).

use std::collections::BTreeMap;

use super::points::{ends, point_xy};
use super::{Link, Network, damaged, fault};
use crate::error::{Result, SqlState};
use crate::geometry::model::{Body, Collection, Curve, Geom, Surface};

/// A part of a collection that makes a network primitive.
enum Part {
    /// A point, a node.
    Point(Geom),
    /// A curve, a link.
    Curve(Geom),
}

/// The points and curves of `g`, in order, nested collections opened; an
/// empty one left out.
///
/// # Errors
///
/// SQLSTATE 2FF02 (invalid argument) of a surface or solid.
fn parts(g: &Geom) -> Result<Vec<Part>> {
    let mut out = Vec::new();
    // A stack of the bodies still to open, the next one last, so nesting
    // takes no recursion.
    let mut stack = vec![g.body.clone()];
    while let Some(body) = stack.pop() {
        if body.is_empty() {
            continue;
        }
        match body {
            Body::Point(c) => out.push(Part::Point(g.part(Body::Point(c)))),
            Body::Curve(c) => out.push(Part::Curve(g.part(Body::Curve(c)))),
            Body::Surface(_) | Body::Solid(_) => {
                let msg = "a surface or solid of the collection makes no node or link";
                return Err(fault("§12.3.16 Description 2h iii 1", SqlState::SPATIAL_INVALID_ARGUMENT, msg));
            }
            Body::Collection(c) => stack.extend(members(c).into_iter().rev()),
        }
    }
    Ok(out)
}

/// The members of collection `c`, each a body.
fn members(c: Collection) -> Vec<Body> {
    match c {
        Collection::Geometry(bodies) => bodies,
        Collection::MultiPoint(points) => points.into_iter().map(Body::Point).collect(),
        Collection::MultiCurve(curves) => curves.into_iter().map(Body::Curve).collect(),
        Collection::MultiLineString(lines) => lines.into_iter().map(|l| Body::Curve(Curve::LineString(l))).collect(),
        Collection::MultiSurface(surfaces) => surfaces.into_iter().map(Body::Surface).collect(),
        Collection::MultiPolygon(polygons) => polygons.into_iter().map(|r| Body::Surface(Surface::Polygon(r))).collect(),
    }
}

/// The nodes of a network being made, by position.
#[derive(Default)]
struct Nodes {
    /// Each node's identifier, by the bits of its x and y.
    at: BTreeMap<[u64; 2], i32>,
}

impl Nodes {
    /// The node at point `p` of `net`, made if there is none.
    fn of(&mut self, net: &mut Network, p: Geom) -> Result<i32> {
        let xy = point_xy(&p).ok_or_else(|| damaged("a node's point is empty"))?;
        let key = xy.map(f64::to_bits);
        if let Some(id) = self.at.get(&key) {
            return Ok(*id);
        }
        let id = net.new_node_id()?;
        net.nodes.insert(id, Some(p));
        self.at.insert(key, id);
        Ok(id)
    }
}

impl Network {
    /// [§12.3.16 Description 2h iii] The nodes and links of collection
    /// `g` added to the network, which is empty.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF02 of a surface or solid; a curve's that is not well
    /// formed.
    pub fn add_geometry(&mut self, g: &Geom) -> Result<()> {
        let mut nodes = Nodes::default();
        for part in parts(g)? {
            match part {
                Part::Point(p) => {
                    nodes.of(self, p)?;
                }
                Part::Curve(c) => {
                    let (s, e) = ends(&c)?.ok_or_else(|| damaged("an empty curve has no ends"))?;
                    let (start, end) = (nodes.of(self, s)?, nodes.of(self, e)?);
                    let id = self.new_link_id()?;
                    self.links.insert(id, Link { start, end, geometry: Some(c) });
                }
            }
        }
        Ok(())
    }
}
