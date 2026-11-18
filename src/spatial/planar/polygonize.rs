//! Polygons from rings, the step ST_BdPolyFromText, ST_BdPolyFromWKB,
//! ST_BdMPolyFromText and ST_BdMPolyFromWKB leave to an
//! implementation-dependent algorithm [ISO/IEC 13249-3:2016 (DIS, SC32
//! N2593) §8.3.9 Description 4d, §8.3.10, §10.6.7 Description 4d,
//! §10.6.8]. The rings are noded into one graph, each raising the depth
//! inside itself by one; a face inside an odd number of rings is
//! interior. So a ring inside an exterior ring is an interior ring, a
//! ring inside that an exterior ring again, and the rings' order and
//! turning directions do not matter.

use super::Point;
use super::assemble::{Selection, assemble};
use super::graph::{Graph, Path, Role};
use super::operand::{closed, raise};
use super::shape::Geometry;
use super::valid::is_linear_ring;
use crate::error::{Code, Result, SqlError};

fn invalid_argument(clause: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[ISO/IEC 13249-3:2016 {clause}] invalid argument"))
}

/// The polygons the rings bound, as an ST_MultiPolygon value.
///
/// # Errors
///
/// SQLCODE -301 if a ring is not a linear ring [§10.6.7 Description 4c];
/// -910 if noding does not settle.
pub fn multipolygon_from_rings(rings: &[Vec<Point>]) -> Result<Geometry> {
    Ok(match polygons(rings, "§10.6.7 Description 4c")? {
        Geometry::Polygon(polygon) => Geometry::MultiPolygon(vec![polygon]),
        Geometry::Empty => Geometry::MultiPolygon(Vec::new()),
        other @ (Geometry::Point(_)
        | Geometry::Line(_)
        | Geometry::MultiPoint(_)
        | Geometry::MultiLine(_)
        | Geometry::MultiPolygon(_)
        | Geometry::Collection(_)) => other,
    })
}

/// The polygon the rings bound, as an ST_Polygon value.
///
/// # Errors
///
/// SQLCODE -301 if a ring is not a linear ring [§8.3.9 Description 4c]
/// or the rings do not bound exactly one polygon; -910 if noding does not
/// settle.
pub fn polygon_from_rings(rings: &[Vec<Point>]) -> Result<Geometry> {
    let clause = "§8.3.9 Description 4c";
    match polygons(rings, clause)? {
        polygon @ Geometry::Polygon(_) => Ok(polygon),
        Geometry::Empty
        | Geometry::Point(_)
        | Geometry::Line(_)
        | Geometry::MultiPoint(_)
        | Geometry::MultiLine(_)
        | Geometry::MultiPolygon(_)
        | Geometry::Collection(_) => Err(invalid_argument(clause)),
    }
}

fn polygons(rings: &[Vec<Point>], clause: &str) -> Result<Geometry> {
    if rings.iter().any(|ring| !is_linear_ring(ring)) {
        return Err(invalid_argument(clause));
    }
    if rings.is_empty() {
        return Ok(Geometry::Empty);
    }
    let paths: Vec<Path> =
        rings.iter().map(|ring| Path { input: 0, role: Role::Ring(raise(ring, true)), vertices: closed(ring) }).collect();
    let graph = Graph::build(&paths)?;
    let faces = graph.cycles.iter().map(|cycle| cycle.depth.first().is_some_and(|depth| depth & 1_i32 == 1_i32)).collect();
    let selection = Selection { faces, lines: vec![false; graph.edges.len()], points: vec![false; graph.nodes.len()] };
    assemble(&graph, &selection)
}
