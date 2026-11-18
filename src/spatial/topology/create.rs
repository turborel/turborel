//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.18] ST_CreateTopoGeo: a
//! topology made from a collection of geometry values. The collection's
//! points, curves and the rings of its surfaces are noded: every curve is
//! split where it crosses or touches another or passes through a point
//! [Description 2i v 1]. Each point, each end of a curve and each place
//! where other than two pieces meet becomes a node; the pieces between
//! nodes become edges, each added as ST_AddEdgeModFace adds one, which
//! makes the faces. Identifiers are given in the order of the nodes'
//! positions and of the edges found, from 1.

use super::noding::{Graph, node};
use super::predicates::same;
use super::{Node, Point, Topology, UNIVERSAL_FACE, fault, linestring};
use crate::error::{Result, SqlState};
use crate::geometry::model::{Body, Collection, Dims, Geom, Surface};

/// The rule of the checks.
const RULE: &str = "§11.3.18 Description 2i";

/// The points and the lines of the linear form of `g`, of x and y,
/// gathered into `points` and `lines`.
fn gather(g: &Geom, points: &mut Vec<Point>, lines: &mut Vec<Vec<Point>>) -> Result<()> {
    let linear = g.linear()?.xy();
    let xy = |c: &Vec<f64>| -> Vec<Point> { c.as_chunks::<2>().0.to_vec() };
    match &linear.body {
        Body::Point(c) => points.extend(xy(c)),
        Body::Curve(_) => lines.push(super::line_of(&linear)?),
        Body::Surface(Surface::Polygon(rings)) => lines.extend(rings.iter().map(xy)),
        Body::Collection(Collection::MultiPoint(ps)) => points.extend(ps.iter().flat_map(xy)),
        Body::Collection(Collection::MultiLineString(ls)) => lines.extend(ls.iter().map(xy)),
        Body::Collection(Collection::MultiPolygon(polys)) => lines.extend(polys.iter().flatten().map(xy)),
        Body::Collection(Collection::Geometry(parts)) => {
            for part in parts {
                gather(&linear.part(part.clone()), points, lines)?;
            }
        }
        Body::Surface(_) | Body::Solid(_) | Body::Collection(_) => {
            return Err(fault(
                RULE,
                SqlState::SPATIAL_INVALID_ARGUMENT,
                &format!("no topology is made of an {}", linear.kind().sql_name()),
            ));
        }
    }
    Ok(())
}

/// [Description 2i i to iv] That `topo` is empty but its universal face.
fn empty(topo: &Topology) -> Result<()> {
    if !topo.nodes.is_empty() || !topo.edges.is_empty() || topo.faces.len() > 1 {
        return Err(fault(RULE, SqlState::SPATIAL_NON_EMPTY_VIEW, "the topology is not empty"));
    }
    if topo.faces.keys().any(|f| *f != UNIVERSAL_FACE) {
        return Err(fault(&format!("{RULE} iv"), SqlState::SPATIAL_INVALID_UNIVERSAL_FACE, "the only face is not the universal face"));
    }
    Ok(())
}

/// [§11.3.18] ST_CreateTopoGeo: `topo`, which has only its universal
/// face, made the topology of `collection`.
///
/// # Errors
///
/// SQLSTATE 2FF42 (non-empty view) if it has a node, an edge or a face but
/// the universal one; 2FF61 (invalid universal face) if its one face is
/// another; 2FF02 of a part that is a solid; SQLCODE -900 of a curve no
/// linearizer takes.
pub fn create(topo: &mut Topology, collection: &Geom) -> Result<()> {
    empty(topo)?;
    let (mut points, mut lines) = (Vec::new(), Vec::new());
    gather(collection, &mut points, &mut lines)?;
    let graph: Graph = node(&points, &lines)?;
    topo.srid = Some(collection.srid);
    let mut ids: Vec<(Point, i32)> = Vec::new();
    for p in &graph.nodes {
        let id = topo.new_node_id()?;
        let geometry = Geom::new(collection.srid, Dims::Xy, Body::Point(p.to_vec()))?;
        topo.nodes.insert(id, Node { geometry, containing_face: Some(UNIVERSAL_FACE) });
        ids.push((*p, id));
    }
    let id_of = |p: Point| ids.iter().find(|(q, _)| same(*q, p)).map(|(_, id)| *id);
    for line in &graph.edges {
        let ends = line.first().copied().and_then(id_of).zip(line.last().copied().and_then(id_of));
        let Some((start, end)) = ends else { continue };
        let curve = linestring(line.iter().flatten().copied().collect(), (collection.srid, Dims::Xy))?;
        topo.insert_edge((start, end), &curve, super::insert::Faces::Modify)?;
    }
    Ok(())
}
