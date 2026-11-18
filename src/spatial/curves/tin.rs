//! Triangulating a TIN from its elements, ISO/IEC 13249-3:2016 (DIS, SC32
//! N2593) §8.6.2 D10 to D15 and §8.6.3 D4 and D5, which leave the element
//! types' effects implementation defined. Here: points and spots are
//! vertices; breaklines, soft breaks and control contours are vertices
//! and constrained edges; a boundary keeps only what it encloses; voids,
//! break voids, drape voids and holes cut out what they enclose; stop
//! lines remove the triangles they pass through (ISO 19107 GM_Tin). Drape
//! voids, holes and a boundary without z take z from the surface the
//! other elements make. The maximum side length peels long-sided
//! triangles off the outside (§8.6.1 D6, an implementation-defined
//! choice).

use super::cdt::Mesh;
use super::fault::{Fault, fault};
use super::numeric::{HALF, V3, norm, sub};
use super::types::Coord;
use crate::error::Result;
use std::collections::HashMap;

/// One third, for triangles' centroids.
const THIRD_OF: f64 = 1.0 / 3.0;

/// The predefined TIN element types (§17.1.1 D4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ElementKind {
    /// 'random points'.
    RandomPoints,
    /// 'group spot'.
    GroupSpot,
    /// 'boundary'.
    Boundary,
    /// 'breakline'.
    Breakline,
    /// 'soft break'.
    SoftBreak,
    /// 'control contour'.
    ControlContour,
    /// 'break void'.
    BreakVoid,
    /// 'drape void'.
    DrapeVoid,
    /// 'void'.
    Void,
    /// 'hole'.
    Hole,
    /// 'stop line'.
    StopLine,
}

/// Every element type, in the order of §17.1.1 D4.
pub const KINDS: [ElementKind; 11] = [
    ElementKind::RandomPoints,
    ElementKind::GroupSpot,
    ElementKind::Boundary,
    ElementKind::Breakline,
    ElementKind::SoftBreak,
    ElementKind::ControlContour,
    ElementKind::BreakVoid,
    ElementKind::DrapeVoid,
    ElementKind::Void,
    ElementKind::Hole,
    ElementKind::StopLine,
];

impl ElementKind {
    /// The type's name as ST_ElementType gives it.
    pub fn name(self) -> &'static str {
        match self {
            ElementKind::RandomPoints => "random points",
            ElementKind::GroupSpot => "group spot",
            ElementKind::Boundary => "boundary",
            ElementKind::Breakline => "breakline",
            ElementKind::SoftBreak => "soft break",
            ElementKind::ControlContour => "control contour",
            ElementKind::BreakVoid => "break void",
            ElementKind::DrapeVoid => "drape void",
            ElementKind::Void => "void",
            ElementKind::Hole => "hole",
            ElementKind::StopLine => "stop line",
        }
    }

    /// The type's well-known text keyword (§5.1.67 `<tin element type>`).
    pub fn keyword(self) -> &'static str {
        match self {
            ElementKind::RandomPoints => "POINTS",
            ElementKind::GroupSpot => "GROUPSPOT",
            ElementKind::Boundary => "BOUNDARY",
            ElementKind::Breakline => "BREAKLINE",
            ElementKind::SoftBreak => "SOFTBREAK",
            ElementKind::ControlContour => "CONTROLCONTOUR",
            ElementKind::BreakVoid => "BREAKVOID",
            ElementKind::DrapeVoid => "DRAPEVOID",
            ElementKind::Void => "VOID",
            ElementKind::Hole => "HOLE",
            ElementKind::StopLine => "STOPLINE",
        }
    }

    /// The type named `name`, matched without regard to case.
    pub fn from_name(name: &str) -> Option<ElementKind> {
        KINDS.into_iter().find(|k| k.name().eq_ignore_ascii_case(name.trim()))
    }

    /// The type of well-known text keyword `word`, matched without
    /// regard to case.
    pub fn from_keyword(word: &str) -> Option<ElementKind> {
        KINDS.into_iter().find(|k| k.keyword().eq_ignore_ascii_case(word))
    }

    /// Whether the element's parts are closed rings around an area.
    pub fn is_area(self) -> bool {
        matches!(self, ElementKind::Boundary | ElementKind::BreakVoid | ElementKind::DrapeVoid | ElementKind::Void | ElementKind::Hole)
    }

    /// Whether the element's consecutive vertices are constrained edges.
    fn constrains(self) -> bool {
        !matches!(self, ElementKind::RandomPoints | ElementKind::GroupSpot | ElementKind::StopLine)
    }

    /// Whether the element's vertices take z from the surface.
    fn drapes(self, has_z: bool) -> bool {
        match self {
            ElementKind::DrapeVoid | ElementKind::Hole => true,
            ElementKind::Boundary => !has_z,
            ElementKind::RandomPoints
            | ElementKind::GroupSpot
            | ElementKind::Breakline
            | ElementKind::SoftBreak
            | ElementKind::ControlContour
            | ElementKind::BreakVoid
            | ElementKind::Void
            | ElementKind::StopLine => false,
        }
    }
}

/// An element's geometry as point lists: its points, its lines or its
/// rings, as its kind says.
#[derive(Clone, Debug, PartialEq)]
pub struct Constraint {
    /// The element type.
    pub kind: ElementKind,
    /// The point lists.
    pub parts: Vec<Vec<Coord>>,
    /// Whether the coordinates carry z.
    pub has_z: bool,
}

/// The triangles, counterclockwise, of the TIN `elements` define, held
/// to `max_side` along the outside.
///
/// # Errors
///
/// SQLSTATE 2FF02 for a maximum side length not positive, 2FF74 for an
/// area element whose ring does not close, 2FF71 when fewer than three
/// distinct vertices remain.
pub fn triangulate(elements: &[Constraint], max_side: Option<f64>) -> Result<Vec<[Coord; 3]>> {
    if max_side.is_some_and(|m| !m.is_finite() || m <= 0.0_f64) {
        return Err(fault(Fault::InvalidArgument, "§8.6.4", "the maximum side length is positive"));
    }
    for element in elements.iter().filter(|e| e.kind.is_area()) {
        if element.parts.iter().any(|ring| ring.first().map(|p| p.space()) != ring.last().map(|p| p.space()) || ring.len() < 4) {
            return Err(fault(Fault::InvalidGeometry, "§17.1.1", &format!("a {} element's ring does not close", element.kind.name())));
        }
    }
    let base = surface(elements.iter().filter(|e| !e.kind.drapes(e.has_z)))?;
    let draped: Vec<Constraint> = elements.iter().map(|e| drape(e, &base)).collect();
    let mesh = build(&draped)?;
    let mut kept = keep(&mesh, &draped);
    if let Some(limit) = max_side {
        peel(&mut kept, limit);
    }
    Ok(kept)
}

/// The mesh of the given elements, constraints in, legalized.
fn surface<'a>(elements: impl Iterator<Item = &'a Constraint> + Clone) -> Result<Mesh> {
    let (low, high) = bounds(elements.clone().flat_map(|e| e.parts.iter().flatten()));
    let mut mesh = Mesh::new(low, high);
    let mut edges = Vec::new();
    for element in elements {
        for part in &element.parts {
            let ids: Vec<usize> = part.iter().map(|p| mesh.insert(p.space())).collect();
            if element.kind.constrains() {
                edges.extend(ids.windows(2).filter_map(|w| if let [a, b] = w { Some((*a, *b)) } else { None }));
            }
        }
    }
    for (a, b) in edges {
        mesh.constrain(a, b)?;
    }
    mesh.legalize();
    Ok(mesh)
}

fn build(elements: &[Constraint]) -> Result<Mesh> {
    let considered = elements.iter().filter(|e| e.kind != ElementKind::StopLine);
    let mesh = surface(considered)?;
    if mesh.vertices().len() < 3 || mesh.triangles().is_empty() {
        return Err(fault(Fault::ThreePointsRequired, "§8.6.3", "a TIN needs three vertices not on one line"));
    }
    Ok(mesh)
}

fn bounds<'a>(points: impl Iterator<Item = &'a Coord>) -> ([f64; 2], [f64; 2]) {
    let (mut low, mut high) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in points {
        low = [low[0].min(p.x), low[1].min(p.y)];
        high = [high[0].max(p.x), high[1].max(p.y)];
    }
    if low[0] > high[0] { ([0.0; 2], [1.0; 2]) } else { (low, high) }
}

/// The element with z taken from the base surface when it drapes.
fn drape(element: &Constraint, base: &Mesh) -> Constraint {
    if !element.kind.drapes(element.has_z) {
        return element.clone();
    }
    let parts = element.parts.iter().map(|part| part.iter().map(|p| Coord { z: height(base, p.x, p.y), ..*p }).collect()).collect();
    Constraint { parts, has_z: true, ..element.clone() }
}

/// The surface's z at (x, y): in its triangle, or the nearest vertex's
/// outside it, zero with no vertices.
fn height(mesh: &Mesh, x: f64, y: f64) -> f64 {
    let points = mesh.vertices();
    for triangle in mesh.triangles() {
        let corners = triangle.map(|v| points.get(v).copied().unwrap_or_default());
        if let Some(found) = barycentric(corners, x, y) {
            return found;
        }
    }
    let distance = |[px, py, _]: &V3| (px - x).hypot(py - y);
    let nearest = points.iter().min_by(|p, q| distance(p).total_cmp(&distance(q)));
    nearest.map_or(0.0, |[_, _, pz]| *pz)
}

fn barycentric(corners: [V3; 3], x: f64, y: f64) -> Option<f64> {
    let [[ax, ay, az], [bx, by, bz], [cx, cy, cz]] = corners;
    let det = (bx - ax).mul_add(cy - ay, -((by - ay) * (cx - ax)));
    if det.abs() <= 0.0_f64 {
        return None;
    }
    let l1 = (bx - x).mul_add(cy - y, -((by - y) * (cx - x))) / det;
    let l2 = (cx - x).mul_add(ay - y, -((cy - y) * (ax - x))) / det;
    let l3 = 1.0_f64 - l1 - l2;
    let slack = -super::numeric::POINT_TOLERANCE;
    (l1 >= slack && l2 >= slack && l3 >= slack).then(|| l1.mul_add(az, l2.mul_add(bz, l3 * cz)))
}

/// The mesh's triangles that the area elements and stop lines keep.
fn keep(mesh: &Mesh, elements: &[Constraint]) -> Vec<[Coord; 3]> {
    let points = mesh.vertices();
    let boundaries: Vec<&Constraint> = elements.iter().filter(|e| e.kind == ElementKind::Boundary).collect();
    let mut out = Vec::new();
    for t in mesh.triangles() {
        let corners = t.map(|v| points.get(v).copied().unwrap_or_default());
        let [a, b, c] = corners;
        let middle = [THIRD_OF * (a[0] + b[0] + c[0]), THIRD_OF * (a[1] + b[1] + c[1])];
        let outside = !boundaries.is_empty() && !boundaries.iter().any(|e| encloses(e, middle));
        let voided = elements.iter().any(|e| e.kind.is_area() && e.kind != ElementKind::Boundary && encloses(e, middle));
        let stopped = elements.iter().any(|e| e.kind == ElementKind::StopLine && e.parts.iter().any(|line| pierces(line, corners)));
        if !outside && !voided && !stopped {
            out.push(corners.map(|p| Coord::at(p, 0.0)));
        }
    }
    out
}

/// Whether the element's rings enclose the point, by the even-odd rule.
fn encloses(element: &Constraint, p: [f64; 2]) -> bool {
    let [x, y] = p;
    let mut inside = false;
    for ring in &element.parts {
        for pair in ring.windows(2) {
            if let [a, b] = pair
                && (a.y > y) != (b.y > y)
                && x < (b.x - a.x) * (y - a.y) / (b.y - a.y) + a.x
            {
                inside = !inside;
            }
        }
    }
    inside
}

/// Whether the polyline passes through the triangle's interior.
fn pierces(line: &[Coord], corners: [V3; 3]) -> bool {
    let [first, second, third] = corners;
    let edges = [(first, second), (second, third), (third, first)];
    let inside = |point: &Coord| {
        let sides = edges.map(|(from, to)| side(from, to, point.x, point.y));
        sides.iter().all(|v| *v > 0.0_f64) || sides.iter().all(|v| *v < 0.0_f64)
    };
    line.iter().any(inside)
        || line.windows(2).any(|pair| {
            let [start, end] = <[Coord; 2]>::try_from(pair).unwrap_or_default();
            let midpoint = Coord::xy(HALF * (start.x + end.x), HALF * (start.y + end.y));
            inside(&midpoint) || edges.iter().any(|(from, to)| proper(start, end, *from, *to))
        })
}

fn side(a: V3, b: V3, x: f64, y: f64) -> f64 {
    (b[0] - a[0]).mul_add(y - a[1], -((b[1] - a[1]) * (x - a[0])))
}

fn proper(p: Coord, q: Coord, u: V3, v: V3) -> bool {
    let pq = |r: V3| (q.x - p.x).mul_add(r[1] - p.y, -((q.y - p.y) * (r[0] - p.x)));
    let (s1, s2) = (pq(u), pq(v));
    let (s3, s4) = (side(u, v, p.x, p.y), side(u, v, q.x, q.y));
    s1 * s2 < 0.0 && s3 * s4 < 0.0
}

/// Removes, until none is left, outside triangles whose outside side is
/// longer than `limit`.
fn peel(triangles: &mut Vec<[Coord; 3]>, limit: f64) {
    loop {
        let mut uses: HashMap<([u64; 2], [u64; 2]), usize> = HashMap::new();
        for t in triangles.iter() {
            for (a, b) in sides(t) {
                let count = uses.entry(key(a, b)).or_insert(0);
                *count = count.saturating_add(1);
            }
        }
        let before = triangles.len();
        triangles
            .retain(|t| !sides(t).into_iter().any(|(a, b)| uses.get(&key(a, b)) == Some(&1) && norm(sub(b.space(), a.space())) > limit));
        if triangles.len() == before {
            return;
        }
    }
}

fn sides(t: &[Coord; 3]) -> [(Coord, Coord); 3] {
    let [a, b, c] = *t;
    [(a, b), (b, c), (c, a)]
}

fn key(a: Coord, b: Coord) -> ([u64; 2], [u64; 2]) {
    let (p, q) = ([a.x.to_bits(), a.y.to_bits()], [b.x.to_bits(), b.y.to_bits()]);
    if p <= q { (p, q) } else { (q, p) }
}

#[cfg(test)]
mod tests;
