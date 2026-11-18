//! ST_Boundary [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.17], the
//! closure of the boundary. Points have none [§6.1.1 Description 9]; a
//! curve's is its two ends unless it is closed [§7.1.1 Description 11,
//! 12]; a multicurve's is the ends of an odd number of its elements
//! [§10.3.1 Description 5]; a polygon's is its rings [§8.2.1 Description
//! 9]. A general collection's is the mod 2 union rule applied through
//! the planar graph [§4.2.2], so its surfaces' boundaries and its curves'
//! odd ends outside them remain.

use std::collections::HashMap;

use super::Point;
use super::assemble::{Selection, assemble};
use super::graph::Graph;
use super::label::{self, Location};
use super::noding::deduplicated;
use super::operand::{closed, paths};
use super::segment::same;
use super::shape::Geometry;
use crate::error::Result;

/// The boundary, the empty set where there is none. The method returns
/// the null value for an empty geometry [§5.1.17 Description 2a]; the
/// engine returns the empty set and leaves that to its caller.
///
/// # Errors
///
/// SQLCODE -910 if noding a collection does not settle.
pub fn boundary(geometry: &Geometry) -> Result<Geometry> {
    Ok(match geometry {
        Geometry::Empty | Geometry::Point(_) | Geometry::MultiPoint(_) => Geometry::Empty,
        Geometry::Line(line) => ends(std::slice::from_ref(line)),
        Geometry::MultiLine(lines) => ends(lines),
        Geometry::Polygon(rings) => rings_of(rings.iter()),
        Geometry::MultiPolygon(polygons) => rings_of(polygons.iter().flatten()),
        Geometry::Collection(_) => collection(geometry)?,
    })
}

/// The ends of an odd number of the curves, in order of first meeting.
fn ends(lines: &[Vec<Point>]) -> Geometry {
    let mut order: Vec<Point> = Vec::new();
    let mut counts: HashMap<[u64; 2], usize> = HashMap::new();
    for line in lines {
        let line = deduplicated(line);
        let (Some(first), Some(last)) = (line.first().copied(), line.last().copied()) else { continue };
        if line.len() < 2 || same(first, last) {
            continue;
        }
        for end in [first, last] {
            let count = counts.entry(end.map(|value| (value + 0.0_f64).to_bits())).or_insert(0);
            if *count == 0 {
                order.push(end);
            }
            *count = count.saturating_add(1);
        }
    }
    let odd: Vec<Point> = order
        .into_iter()
        .filter(|end| counts.get(&end.map(|value| (value + 0.0_f64).to_bits())).is_some_and(|count| count & 1 == 1))
        .collect();
    if odd.is_empty() { Geometry::Empty } else { Geometry::MultiPoint(odd) }
}

fn rings_of<'a>(rings: impl Iterator<Item = &'a Vec<Point>>) -> Geometry {
    let mut lines: Vec<Vec<Point>> = rings.filter(|ring| !ring.is_empty()).map(|ring| closed(ring)).collect();
    match lines.len() {
        0 => Geometry::Empty,
        1 => lines.pop().map_or(Geometry::Empty, Geometry::Line),
        _ => Geometry::MultiLine(lines),
    }
}

/// The edges and nodes of the collection's graph located on its
/// boundary.
fn collection(geometry: &Geometry) -> Result<Geometry> {
    let parts = geometry.parts();
    if parts.is_empty() {
        return Ok(Geometry::Empty);
    }
    let graph = Graph::build(&paths(&parts, 0))?;
    let faces = vec![false; graph.cycles.len()];
    let lines: Vec<bool> = (0..graph.edges.len()).map(|id| label::edge(&graph, id, 0) == Location::Boundary).collect();
    let mut selection = Selection { faces, lines, points: Vec::new() };
    let points: Vec<bool> =
        (0..graph.nodes.len()).map(|id| label::node(&graph, id, 0) == Location::Boundary && !selection.covers(&graph, id)).collect();
    selection.points = points;
    assemble(&graph, &selection)
}
