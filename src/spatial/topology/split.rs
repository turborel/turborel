//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.8, §11.3.9]
//! ST_NewEdgesSplit and ST_ModEdgeSplit: an edge split at a new node, into
//! two new edges or into itself and one new edge, each in the direction of
//! the edge split. The curve is cut at the point: the point becomes a
//! vertex of both parts, with the z and m of the given point if it has the
//! edge's dimensions, else interpolated along the segment it lies on.

use super::predicates::{on_segment, same};
use super::{Edge, Node, Point, Topology, damaged, fault, line_of, linestring, point_of};
use crate::error::{Result, SqlState};
use crate::geometry::model::{Body, Coords, Curve, Dims, Geom};

/// The rule of the checks of both routines.
const NEW: &str = "§11.3.8 Description 2f";
/// The rule of ST_ModEdgeSplit's checks.
const MODIFY: &str = "§11.3.9 Description 2f";

/// The points, with all their ordinates, of the linear form of curve `g`.
pub(super) fn coords_of(g: &Geom) -> Result<Coords> {
    match g.linear()?.body {
        Body::Curve(Curve::LineString(c)) => Ok(c),
        Body::Point(_) | Body::Curve(_) | Body::Surface(_) | Body::Solid(_) | Body::Collection(_) => {
            Err(damaged("an edge's geometry is no curve"))
        }
    }
}

/// The ordinates of point `p` within the segment from `a` to `b`, all of
/// `dims`: those of `given` if it has them, else x and y of `p` and the
/// rest interpolated by the distance along the segment.
fn vertex(p: Point, (a, b): (&[f64], &[f64]), given: (&Geom, Dims)) -> Vec<f64> {
    let (point, dims) = given;
    if point.dims == dims
        && let Body::Point(c) = &point.body
    {
        return c.clone();
    }
    let [px, py] = p;
    let along = |q: &[f64]| q.first().zip(q.get(1)).map_or(0.0_f64, |(x, y)| ((x - px) * (x - px) + (y - py) * (y - py)).sqrt());
    let (da, db) = (along(a), along(b));
    let t = if da + db > 0.0_f64 { da / (da + db) } else { 0.0_f64 };
    let mut out = vec![px, py];
    out.extend(a.iter().zip(b).skip(2).map(|(x, y)| x + t * (y - x)));
    out
}

/// Curve `g`, of the points `coords`, cut at `p`, which lies on it but at
/// neither end: the parts before and after.
fn cut(coords: &Coords, dims: Dims, (p, point): (Point, &Geom)) -> Result<(Coords, Coords)> {
    let points: Vec<&[f64]> = coords.chunks_exact(dims.stride()).collect();
    let xy = |q: &[f64]| -> Point { [q.first().copied().unwrap_or_default(), q.get(1).copied().unwrap_or_default()] };
    let k = points
        .windows(2)
        .position(|w| if let [a, b] = w { on_segment(p, xy(a), xy(b)) } else { false })
        .ok_or_else(|| damaged("the point is on no segment of the edge"))?;
    let (a, b) = (points.get(k).copied().unwrap_or_default(), points.get(k.saturating_add(1)).copied().unwrap_or_default());
    let at = vertex(p, (a, b), (point, dims));
    let mut first: Coords = points.iter().take(k.saturating_add(1)).flat_map(|q| q.iter().copied()).collect();
    if !same(xy(a), p) {
        first.extend(&at);
    }
    let mut second = at;
    let rest = points.iter().skip(k.saturating_add(1)).skip_while(|q| same(xy(q), p));
    second.extend(rest.flat_map(|q| q.iter().copied()));
    Ok((first, second))
}

impl Topology {
    /// [§11.3.8, §11.3.9] Edge `edge` split at `point` by a new node: into
    /// two new edges if `new_edges`, else into itself and one new edge; the
    /// new node's identifier.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF43 (non-existent edge); 2FF54 (point not on edge) if the
    /// point is not within the edge's curve; 2FF28 (coincident node); 2FF10.
    pub fn split_edge(&mut self, edge: i32, point: &Geom, new_edges: bool) -> Result<i32> {
        let rule = if new_edges { NEW } else { MODIFY };
        self.same_srid(point, "§11.3.8 Description 3")?;
        let old = self.edge(edge, &format!("{rule} i"))?.clone();
        let p = point_of(point)?;
        let line = line_of(&old.geometry)?;
        let closed = old.start == old.end;
        // [§5.1.53] Within a curve is on it, and off its boundary: its two
        // ends, unless it is closed.
        let at_end = [line.first(), line.last()].into_iter().flatten().any(|q| same(*q, p));
        if !super::predicates::on_line(p, &line) || (at_end && !closed) {
            return Err(fault(&format!("{rule} ii"), SqlState::SPATIAL_POINT_NOT_ON_EDGE, &format!("the point is not within edge {edge}")));
        }
        self.no_coincident_node(p, None, &format!("{rule} iii"))?;
        let (first, second) = cut(&coords_of(&old.geometry)?, old.geometry.dims, (p, point))?;
        let shape = (old.geometry.srid, old.geometry.dims);
        let (geom1, geom2) = (linestring(first, shape)?, linestring(second, shape)?);
        let node = self.new_node_id()?;
        self.nodes.insert(node, Node { geometry: point.clone(), containing_face: None });
        let ends = [old.start, old.end, node];
        let made = Edge { start: node, end: old.end, geometry: geom2, ..old.clone() };
        if new_edges {
            // New identifiers, none of them the split edge's.
            let id1 = self.new_edge_id()?;
            self.edges.remove(&edge);
            let id2 = id1.checked_add(1).ok_or_else(|| damaged("no edge identifier is left"))?;
            self.edges.insert(id1, Edge { end: node, geometry: geom1, ..old });
            self.edges.insert(id2, made);
        } else {
            let id = self.new_edge_id()?;
            self.edges.insert(edge, Edge { end: node, geometry: geom1, ..old });
            self.edges.insert(id, made);
        }
        self.relink(&ends)?;
        Ok(node)
    }
}
