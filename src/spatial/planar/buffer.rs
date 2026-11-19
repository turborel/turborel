//! ST_Buffer [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.30], every
//! point whose distance from the geometry is at most the given distance.
//!
//! The buffer is built from offset pieces, noded together in one graph.
//! Each piece is a convex ring raising the area depth inside itself, so
//! the faces of positive depth are their union and overlaps need no
//! separate union. A segment gives a half strip on each side that needs
//! one, the rectangle from the segment to its offset at the distance. A
//! vertex gives a round join on the side its path turns away from, the
//! wedge of the disc between the two segments' offset normals. An open
//! end gives a half disc, the round cap, and a point gives a disc. The
//! half strips and wedges meet in shared offset points, so on a convex
//! path they tile the band without crossing.
//!
//! Every point within the distance lies in a piece. If its nearest point
//! of the path is inside a segment, it lies in that segment's half strip.
//! If it is a vertex, it lies in the normal cone there, which is the
//! join's wedge or an end's cap. A polygon needs only the side away from
//! its interior, since its own rings add the interior to the union.
//!
//! Arcs are inscribed in their circles, a step of π/16 apart, the
//! implementation-defined approximation of round joins and caps. Their
//! directions come from repeated exact rotation by constants, not the
//! platform's trigonometry, so results are the same everywhere.
//!
//! The clause defines the result for any distance. A distance below
//! zero leaves no point, so the result is the empty set; a distance of
//! zero leaves the geometry itself. The method returns the null value
//! for an empty geometry [§5.1.30 Description 2b i]; the engine returns
//! the empty set and leaves that to its caller.

use std::cmp::Ordering;

use super::Point;
use super::assemble::{Selection, assemble};
use super::graph::{Graph, Path, Role};
use super::measure::norm;
use super::noding::deduplicated;
use super::operand::{closed, paths, raise};
use super::predicates::orient2d;
use super::segment::equal;
use super::shape::Geometry;
use crate::error::{Code, Result, SqlError};

/// The cosine and sine of π/16, the turn between an arc's vertices.
const STEP_COS: f64 = 0.980_785_280_403_230_4;
const STEP_SIN: f64 = 0.195_090_322_016_128_25;
/// The sine of π/64. An arc vertex nearer than a quarter step to the
/// arc's end is left out, so no sliver segment ends an arc.
const QUARTER_STEP_SIN: f64 = 0.049_067_674_327_418_015;
/// Arc vertices in a full circle.
const CIRCLE_STEPS: usize = 32;

/// The buffer of a geometry.
///
/// # Errors
///
/// SQLCODE -301 if the distance is not finite or a segment's length
/// overflows; -910 if noding does not settle.
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
    let rings = paths(&parts, 0).into_iter().filter(|path| matches!(path.role, Role::Ring(_))).collect();
    let mut pieces = Pieces { distance, rings };
    for point in &parts.points {
        pieces.disc(*point);
    }
    for line in &parts.lines {
        pieces.line(&deduplicated(line))?;
    }
    for polygon in &parts.polygons {
        for (index, ring) in polygon.iter().enumerate() {
            let mut ring = deduplicated(&closed(ring));
            // Interior on the left, so the outward side is the right.
            if raise(&ring, index == 0) < 0_i32 {
                ring.reverse();
            }
            pieces.ring(&ring)?;
        }
    }
    let graph = Graph::build(&pieces.rings)?;
    let faces = graph.cycles.iter().map(|cycle| cycle.depth.first().is_some_and(|depth| *depth > 0_i32)).collect();
    assemble(&graph, &Selection { faces, lines: vec![false; graph.edges.len()], points: vec![false; graph.nodes.len()] })
}

/// The convex rings whose union is the buffer.
struct Pieces {
    distance: f64,
    rings: Vec<Path>,
}

/// The unit normals on the left of a path's segments, which have no
/// repeated points.
fn normals(path: &[Point]) -> Result<Vec<Point>> {
    let mut out = Vec::with_capacity(path.len());
    for pair in path.windows(2) {
        let [[ax, ay], [bx, by]] = pair else { continue };
        let length = norm(bx - ax, by - ay);
        if !length.is_finite() || length <= 0.0_f64 {
            return Err(SqlError::new(Code::Data, "[DESIGN] a buffered segment's length must be finite"));
        }
        out.push([-(by - ay) / length, (bx - ax) / length]);
    }
    Ok(out)
}

fn negated([x, y]: Point) -> Point {
    [-x, -y]
}

/// Turns a direction counterclockwise by one step.
fn turn([x, y]: Point) -> Point {
    [x * STEP_COS - y * STEP_SIN, x * STEP_SIN + y * STEP_COS]
}

fn cross([ax, ay]: Point, [bx, by]: Point) -> f64 {
    ax * by - ay * bx
}

/// Whether the collinear path from `a` through `b` to `c` turns back.
fn reverses([ax, ay]: Point, [bx, by]: Point, [cx, cy]: Point) -> bool {
    let behind = |from: f64, at: f64, to: f64| (from < at) == (to < at);
    if equal(ax, bx) { behind(ay, by, cy) } else { behind(ax, bx, cx) }
}

impl Pieces {
    /// The point at the distance from `centre` in unit direction `toward`.
    fn at(&self, [cx, cy]: Point, [dx, dy]: Point) -> Point {
        [cx + self.distance * dx, cy + self.distance * dy]
    }

    fn push(&mut self, mut vertices: Vec<Point>) {
        vertices.extend(vertices.first().copied());
        self.rings.push(Path { input: 0, role: Role::Ring(1), vertices });
    }

    /// The wedge about `centre` turning counterclockwise from unit
    /// direction `from` to `to`, at most half a turn, as one ring.
    fn wedge(&mut self, centre: Point, from: Point, to: Point) {
        let mut vertices = vec![centre, self.at(centre, from)];
        let mut direction = turn(from);
        for _ in 0..CIRCLE_STEPS {
            if cross(direction, to) <= QUARTER_STEP_SIN {
                break;
            }
            vertices.push(self.at(centre, direction));
            direction = turn(direction);
        }
        vertices.push(self.at(centre, to));
        self.push(vertices);
    }

    /// The disc about a point.
    fn disc(&mut self, centre: Point) {
        let mut direction = [1.0_f64, 0.0_f64];
        let mut vertices = Vec::with_capacity(CIRCLE_STEPS.saturating_add(1));
        for _ in 0..CIRCLE_STEPS {
            vertices.push(self.at(centre, direction));
            direction = turn(direction);
        }
        self.push(vertices);
    }

    /// The half strip on the left of the segment from `a` to `b`, whose
    /// left unit normal is `left`.
    fn strip(&mut self, a: Point, b: Point, left: Point) {
        let vertices = vec![a, b, self.at(b, left), self.at(a, left)];
        self.push(vertices);
    }

    /// The join at `b` on the side the path from `a` through `b` to `c`
    /// turns away from, given the left normals of its two segments. Of a
    /// polygon's ring only joins on the right are needed.
    fn join(&mut self, [a, b, c]: [Point; 3], [before, after]: [Point; 2], right_only: bool) {
        match orient2d(a, b, c) {
            Ordering::Greater => self.wedge(b, negated(before), negated(after)),
            Ordering::Less => {
                if !right_only {
                    self.wedge(b, after, before);
                }
            }
            Ordering::Equal => {
                if reverses(a, b, c) {
                    self.wedge(b, negated(before), before);
                }
            }
        }
    }

    /// An open line string's pieces: both half strips of every segment,
    /// a join at every inner vertex and a cap at each end.
    fn line(&mut self, line: &[Point]) -> Result<()> {
        let normals = normals(line)?;
        let (Some(first), Some(last), Some(start), Some(end)) = (line.first(), line.last(), normals.first(), normals.last()) else {
            if let Some(point) = line.first() {
                self.disc(*point);
            }
            return Ok(());
        };
        for (pair, left) in line.windows(2).zip(&normals) {
            if let [a, b] = pair {
                self.strip(*a, *b, *left);
                self.strip(*b, *a, negated(*left));
            }
        }
        for (triple, pair) in line.windows(3).zip(normals.windows(2)) {
            if let ([a, b, c], [before, after]) = (triple, pair) {
                self.join([*a, *b, *c], [*before, *after], false);
            }
        }
        self.wedge(*first, *start, negated(*start));
        self.wedge(*last, negated(*end), *end);
        Ok(())
    }

    /// A closed ring's pieces on its right: the right half strip of every
    /// segment and the joins on the right, where it closes too.
    fn ring(&mut self, ring: &[Point]) -> Result<()> {
        let normals = normals(ring)?;
        let count = normals.len();
        if count < 2 {
            return self.line(ring);
        }
        for (pair, left) in ring.windows(2).zip(&normals) {
            if let [a, b] = pair {
                self.strip(*b, *a, negated(*left));
            }
        }
        for at in 0..count {
            let next = at.saturating_add(1);
            // The vertex after the closing one is the second.
            let after_next = if next < count { next.saturating_add(1) } else { 1 };
            let vertices = (ring.get(at), ring.get(next), ring.get(after_next));
            let wrapped = if next < count { next } else { 0 };
            let sides = (normals.get(at), normals.get(wrapped));
            if let ((Some(a), Some(b), Some(c)), (Some(before), Some(after))) = (vertices, sides) {
                self.join([*a, *b, *c], [*before, *after], true);
            }
        }
        Ok(())
    }
}
