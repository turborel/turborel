//! ST_Buffer [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.30], every
//! point whose distance from the geometry is at most the given distance.
//! The set is the union of the surfaces, a rectangle on each segment and
//! a disc at each vertex and point. Those raw offset rings are noded
//! together in one graph, each raising the area depth inside itself, and
//! the faces of positive depth are the result, so their overlaps need no
//! separate union. Discs are 32-sided polygons inscribed in their
//! circles, the implementation-defined approximation of the round joins
//! and ends; their directions come from repeated exact rotation by
//! constants, not the platform's trigonometry, so results are the same
//! everywhere.
//!
//! The clause defines the result for any distance. A distance below
//! zero leaves no point, so the result is the empty set; a distance of
//! zero leaves the geometry itself. The method returns the null value
//! for an empty geometry [§5.1.30 Description 2b i]; the engine returns
//! the empty set and leaves that to its caller.

use std::collections::HashSet;

use super::Point;
use super::assemble::{Selection, assemble};
use super::graph::{Graph, Path, Role};
use super::measure::norm;
use super::operand::{closed, paths};
use super::shape::Geometry;
use crate::error::{Code, Result, SqlError};

/// Segments per quarter circle of a disc.
const QUADRANT_SEGMENTS: usize = 8;
const QUADRANTS: usize = 4;
/// The cosine and sine of π/16, the turn between a disc's vertices.
const STEP_COS: f64 = 0.980_785_280_403_230_4;
const STEP_SIN: f64 = 0.195_090_322_016_128_25;

/// The buffer of a geometry.
///
/// # Errors
///
/// SQLCODE -301 if the distance is not finite; -910 if noding does not
/// settle.
pub fn buffer(geometry: &Geometry, distance: f64) -> Result<Geometry> {
    if !distance.is_finite() {
        return Err(SqlError::new(Code::Data, "[DESIGN] a buffer distance must be finite"));
    }
    let parts = geometry.parts();
    if parts.is_empty() || distance < 0.0_f64 {
        return Ok(Geometry::Empty);
    }
    if distance <= 0.0_f64 {
        return Ok(geometry.clone());
    }
    let mut rings: Vec<Path> = paths(&parts, 0).into_iter().filter(|path| matches!(path.role, Role::Ring(_))).collect();
    let closed_rings: Vec<Vec<Point>> = parts.polygons.iter().flatten().map(|ring| closed(ring)).collect();
    let polylines: Vec<&Vec<Point>> = parts.lines.iter().chain(&closed_rings).collect();
    let mut seen = HashSet::new();
    let vertices = parts.points.iter().chain(polylines.iter().flat_map(|line| line.iter()));
    for vertex in vertices {
        if seen.insert(vertex.map(|value| (value + 0.0_f64).to_bits())) {
            rings.push(raised(disc(*vertex, distance)));
        }
    }
    for line in polylines {
        for pair in line.windows(2) {
            if let [a, b] = pair
                && let Some(rectangle) = rectangle(*a, *b, distance)
            {
                rings.push(raised(rectangle));
            }
        }
    }
    let graph = Graph::build(&rings)?;
    let faces = graph.cycles.iter().map(|cycle| cycle.depth.first().is_some_and(|depth| *depth > 0_i32)).collect();
    assemble(&graph, &Selection { faces, lines: vec![false; graph.edges.len()], points: vec![false; graph.nodes.len()] })
}

fn raised(vertices: Vec<Point>) -> Path {
    Path { input: 0, role: Role::Ring(1), vertices }
}

/// Turns a direction counterclockwise by one step.
fn turn([x, y]: Point) -> Point {
    [x * STEP_COS - y * STEP_SIN, x * STEP_SIN + y * STEP_COS]
}

/// A closed counterclockwise polygon inscribed in the circle of radius
/// `radius` about `centre`.
fn disc([cx, cy]: Point, radius: f64) -> Vec<Point> {
    let mut direction = [1.0_f64, 0.0_f64];
    let mut ring = Vec::with_capacity(QUADRANT_SEGMENTS.saturating_mul(QUADRANTS).saturating_add(1));
    for _ in 0..QUADRANT_SEGMENTS.saturating_mul(QUADRANTS) {
        let [dx, dy] = direction;
        ring.push([cx + radius * dx, cy + radius * dy]);
        direction = turn(direction);
    }
    ring.extend(ring.first().copied());
    ring
}

/// The closed counterclockwise rectangle of half-width `radius` along
/// the segment from `a` to `b`, or nothing for a segment of no length.
fn rectangle(a: Point, b: Point, radius: f64) -> Option<Vec<Point>> {
    let ([ax, ay], [bx, by]) = (a, b);
    let length = norm(bx - ax, by - ay);
    if length <= 0.0_f64 || !length.is_finite() {
        return None;
    }
    let [nx, ny] = [-(by - ay) / length * radius, (bx - ax) / length * radius];
    Some(vec![[ax - nx, ay - ny], [bx - nx, by - ny], [bx + nx, by + ny], [ax + nx, ay + ny], [ax - nx, ay - ny]])
}
