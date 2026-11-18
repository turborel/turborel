//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.5, §11.3.16] The rings
//! of signed edges round faces, traced on the next left and next right
//! edges, and what is made of them: the edges of a face in
//! counterclockwise order, and the geometry of a face.
//!
//! A ring walks each edge with the face on its left: forward if the face
//! is its left face, backward if its right. An edge with the face on both
//! sides is walked both ways in one ring. Such an edge bounds no area, so
//! a face's geometry leaves it out: taking out the two walks of such an
//! edge from a ring leaves two rings, or one if the edge dangled.

use std::collections::{BTreeMap, BTreeSet};

use super::predicates::{self, Side, counterclockwise, same, side};
use super::{Point, Topology, UNIVERSAL_FACE, damaged, fault, line_of};
use crate::error::{Result, SqlState};
use crate::geometry::model::{Body, Dims, Geom, Surface};

/// The rings of one face's geometry: those whose area is on their left,
/// and the holes.
#[derive(Debug, Clone, Default)]
pub(super) struct Shape {
    /// The rings that turn counterclockwise.
    pub shells: Vec<Vec<Point>>,
    /// The rings that turn clockwise.
    pub holes: Vec<Vec<Point>>,
}

impl Shape {
    /// Where `p` lies against the face: inside its one shell and outside
    /// its holes. A face of no single shell holds no point.
    pub fn side(&self, p: Point) -> Side {
        let [shell] = self.shells.as_slice() else { return Side::Outside };
        match side(p, shell) {
            Side::Inside => {}
            other @ (Side::On | Side::Outside) => return other,
        }
        for hole in &self.holes {
            match side(p, hole) {
                Side::Inside => return Side::Outside,
                Side::On => return Side::On,
                Side::Outside => {}
            }
        }
        Side::Inside
    }
}

impl Topology {
    /// The signed edge after `signed` round the face on its left.
    pub(super) fn next(&self, signed: i32) -> Result<i32> {
        let edge =
            self.edges.get(&signed.saturating_abs()).ok_or_else(|| damaged(&format!("a ring passes edge {signed}, which is not there")))?;
        Ok(if signed > 0 { edge.next_left } else { edge.next_right })
    }

    /// The face on the left of signed edge `signed`.
    pub(super) fn left_of(&self, signed: i32) -> Result<i32> {
        let edge = self.edges.get(&signed.saturating_abs()).ok_or_else(|| damaged(&format!("no edge {signed}")))?;
        Ok(if signed > 0 { edge.left_face } else { edge.right_face })
    }

    /// The ring of signed edges from `signed` round the face on its left.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if the pointers make no ring through it.
    pub(super) fn ring(&self, signed: i32) -> Result<Vec<i32>> {
        let limit = self.edges.len().saturating_mul(2).saturating_add(1);
        let mut ring = vec![signed];
        let mut at = self.next(signed)?;
        while at != signed {
            if ring.len() > limit {
                return Err(damaged(&format!("the next edges from edge {signed} make no ring")));
            }
            ring.push(at);
            at = self.next(at)?;
        }
        Ok(ring)
    }

    /// Every ring of the topology, each with the face on its left, the
    /// rings of each face in the order of their least signed edge.
    pub(super) fn rings(&self) -> Result<Vec<(i32, Vec<i32>)>> {
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        let signed = self.edges.keys().flat_map(|e| [e.checked_neg(), Some(*e)]).flatten().collect::<Vec<_>>();
        for s in signed {
            if seen.contains(&s) {
                continue;
            }
            let ring = self.ring(s)?;
            seen.extend(ring.iter().copied());
            out.push((self.left_of(s)?, ring));
        }
        Ok(out)
    }

    /// The points of signed edges `ring`, walked in turn, closed.
    pub(super) fn ring_points(&self, ring: &[i32]) -> Result<Vec<Point>> {
        let mut points: Vec<Point> = Vec::new();
        for signed in ring {
            let edge = self.edges.get(&signed.saturating_abs()).ok_or_else(|| damaged(&format!("no edge {signed}")))?;
            let mut line = line_of(&edge.geometry)?;
            if *signed < 0_i32 {
                line.reverse();
            }
            let skip = usize::from(points.last().zip(line.first()).is_some_and(|(a, b)| same(*a, *b)));
            points.extend(line.into_iter().skip(skip));
        }
        if let (Some(first), Some(last)) = (points.first().copied(), points.last().copied())
            && !same(first, last)
        {
            points.push(first);
        }
        Ok(points)
    }

    /// The shape of every face but the universal one, from its rings with
    /// the edges it lies on both sides of taken out.
    pub(super) fn shapes(&self) -> Result<BTreeMap<i32, Shape>> {
        let mut out: BTreeMap<i32, Shape> = self.faces.keys().filter(|f| **f != UNIVERSAL_FACE).map(|f| (*f, Shape::default())).collect();
        for (face, ring) in self.rings()? {
            let Some(shape) = out.get_mut(&face) else { continue };
            for part in unspiked(ring) {
                let points = self.ring_points(&part)?;
                if counterclockwise(&points) {
                    shape.shells.push(points);
                } else {
                    shape.holes.push(points);
                }
            }
        }
        Ok(out)
    }

    /// The shape of face `face`.
    pub(super) fn shape(&self, face: i32) -> Result<Shape> {
        Ok(self.shapes()?.remove(&face).unwrap_or_default())
    }
}

/// The rings left of `ring` when each edge walked both ways in it is taken
/// out, with both its walks: the part between the two walks becomes a
/// ring of its own, so a bridge to a hole leaves the shell and the hole,
/// and a dangling edge leaves nothing of itself.
fn unspiked(ring: Vec<i32>) -> Vec<Vec<i32>> {
    let mut work = vec![ring];
    let mut out = Vec::new();
    while let Some(r) = work.pop() {
        let both = r.iter().enumerate().find_map(|(i, s)| r.iter().position(|t| t.checked_neg() == Some(*s)).map(|j| (i.min(j), i.max(j))));
        match both {
            Some((i, j)) => {
                let inner: Vec<i32> = r.iter().skip(i.saturating_add(1)).take(j.saturating_sub(i).saturating_sub(1)).copied().collect();
                let outer: Vec<i32> = r.iter().take(i).chain(r.iter().skip(j.saturating_add(1))).copied().collect();
                work.extend([inner, outer].into_iter().filter(|p| !p.is_empty()));
            }
            None => out.push(r),
        }
    }
    out
}

/// The rotation of `ring` that starts at its least signed edge.
fn from_least(ring: &[i32]) -> Vec<i32> {
    let at = ring.iter().enumerate().min_by_key(|(_, s)| **s).map_or(0, |(i, _)| i);
    ring.iter().skip(at).chain(ring.iter().take(at)).copied().collect()
}

/// That `face` is a face of `topo` other than the universal one.
fn bounded(topo: &Topology, face: i32, rule: &str) -> Result<()> {
    if face == UNIVERSAL_FACE {
        return Err(fault(rule, SqlState::SPATIAL_UNIVERSAL_FACE_HAS_NO_GEOMETRY, "the universal face has no geometry"));
    }
    if !topo.faces.contains_key(&face) {
        return Err(fault(rule, SqlState::SPATIAL_NON_EXISTENT_FACE, &format!("no face {face}")));
    }
    Ok(())
}

/// [§11.3.5] ST_GetFaceEdges: the signed edges bounding face `face`, each
/// negative if the face is on its right, counterclockwise round the face.
/// Reading: a face of several rings gives each ring in turn, each from its
/// least signed edge, the rings in the order of those, so the least of all
/// comes first [Description 2e i 2]. An edge with the face on both sides
/// is given twice, once each way, as the ring walks it.
///
/// # Errors
///
/// SQLSTATE 2FF44 (non-existent face) if there is no such face; SQLCODE
/// -916 if the pointers make no ring.
pub fn face_edges(topo: &Topology, face: i32) -> Result<Vec<i32>> {
    if !topo.faces.contains_key(&face) {
        return Err(fault("§11.3.5 Description 2e", SqlState::SPATIAL_NON_EXISTENT_FACE, &format!("no face {face}")));
    }
    let mut rings: Vec<Vec<i32>> = topo.rings()?.into_iter().filter(|(f, _)| *f == face).map(|(_, r)| from_least(&r)).collect();
    rings.sort_by_key(|r| r.first().copied());
    Ok(rings.concat())
}

/// [§11.3.16] ST_GetFaceGeometry: the polygon of face `face`, of x and y,
/// its exterior ring the one ring that turns counterclockwise and its
/// interior rings those that turn clockwise; the empty polygon if no valid
/// polygon is made of its rings [Description 2e iii 4B].
///
/// # Errors
///
/// SQLSTATE 2FF60 (universal face has no geometry) of face 0; 2FF44
/// (non-existent face) if there is no such face.
pub fn face_geometry(topo: &Topology, face: i32) -> Result<Geom> {
    bounded(topo, face, "§11.3.16 Description 2e")?;
    let shape = topo.shape(face)?;
    Ok(polygon(&shape, topo.srid_or_unknown()).unwrap_or_else(|| empty_polygon(topo.srid_or_unknown())))
}

/// The polygon of `shape`, if it has one shell and every hole is inside it.
pub(super) fn polygon(shape: &Shape, srid: i32) -> Option<Geom> {
    let [shell] = shape.shells.as_slice() else { return None };
    let inside = |hole: &Vec<Point>| hole.iter().all(|p| side(*p, shell) != Side::Outside);
    if !shape.holes.iter().all(inside) {
        return None;
    }
    let rings = std::iter::once(shell).chain(&shape.holes).map(|r| r.iter().flatten().copied().collect()).collect();
    Geom::new(srid, Dims::Xy, Body::Surface(Surface::Polygon(rings))).ok()
}

/// The empty polygon of `srid`.
fn empty_polygon(srid: i32) -> Geom {
    Geom { srid, dims: Dims::Xy, body: Body::Surface(Surface::Polygon(Vec::new())) }
}

/// [§11.3.12 Description 2h ix 3E] The minimum bounding rectangle of
/// `shape`, an ST_Polygon; none if it has no shell.
pub(super) fn mbr(shape: &Shape, srid: i32) -> Option<Geom> {
    let [shell] = shape.shells.as_slice() else { return None };
    let [x0, y0, x1, y1] = predicates::bounds(shell)?;
    let ring = vec![x0, y0, x1, y0, x1, y1, x0, y1, x0, y0];
    Geom::new(srid, Dims::Xy, Body::Surface(Surface::Polygon(vec![ring]))).ok()
}
