//! Surfaces of planar patches: triangles, polyhedral surfaces and TINs,
//! ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.4 to §8.6, measured by
//! §8.1.2 to §8.1.9. Areas and centroids add up patch by patch; the
//! boundary, which perimeters measure, is the edges only one patch uses.

use super::fault::{Fault, fault};
use super::geodesy::Planar;
use super::interior::point_on_surface as polygon_point;
use super::numeric::{HALF, Sum, V3, add, axpy, cross, dot, flat, norm, scale, sub};
use super::piece::Metric;
use super::types::{Coord, Curve};
use crate::error::Result;
use std::collections::HashMap;

/// A planar polygon's rings, exterior first, each closed.
pub type Patch = Vec<Vec<Coord>>;

/// One third, for the centroids of triangles.
const THIRD: f64 = 1.0 / 3.0;
/// The points of a triangle's closed ring (§8.4.1 D7).
const TRIANGLE_POINTS: usize = 4;

/// Checks a triangle's rings (§8.4.1): one ring of four points, closed.
///
/// # Errors
///
/// SQLSTATE 2FF66 for an interior ring, 2FF75 for a ring not of four
/// points, 2FF74 for a ring that does not close.
pub fn check_triangle(rings: &[Vec<Coord>]) -> Result<()> {
    if rings.len() > 1 {
        return Err(fault(Fault::TrianglesHaveNoHoles, "§8.4.1", "a triangle has only an exterior ring"));
    }
    let ring = rings.first().map(Vec::as_slice).unwrap_or_default();
    if ring.len() != TRIANGLE_POINTS {
        return Err(fault(Fault::FourPointRing, "§8.4.1", "a triangle's ring has four points, the last equal to the first"));
    }
    if ring.first().map(|p| p.space()) != ring.last().map(|p| p.space()) {
        return Err(fault(Fault::InvalidGeometry, "§8.4.1", "a triangle's ring is not closed"));
    }
    Ok(())
}

/// `v` with its z dropped in the plane metric.
fn lay(v: V3, metric: Metric) -> V3 {
    match metric {
        Metric::Plane => flat(v),
        Metric::Space => v,
    }
}

/// A ring's vector area about `reference`.
fn ring_area(ring: &[Coord], reference: V3, metric: Metric) -> V3 {
    let mut total = [Sum::default(); 3];
    for pair in ring.windows(2) {
        if let [a, b] = pair {
            let c = cross(sub(lay(a.space(), metric), reference), sub(lay(b.space(), metric), reference));
            for (sum, value) in total.iter_mut().zip(c) {
                sum.add(HALF * value);
            }
        }
    }
    total.map(Sum::value)
}

/// A patch's area (exterior less interiors) and its centroid.
pub fn patch_moments(patch: &Patch, metric: Metric) -> (f64, V3) {
    let Some(reference) = patch.first().and_then(|r| r.first()).map(|p| lay(p.space(), metric)) else {
        return (0.0, [0.0; 3]);
    };
    let exterior = patch.first().map_or([0.0_f64; 3], |r| ring_area(r, reference, metric));
    let size = norm(exterior);
    let normal = if size > 0.0_f64 { scale(exterior, 1.0 / size) } else { [0.0_f64, 0.0_f64, 1.0_f64] };
    let (mut area, mut moment) = (0.0_f64, [0.0_f64; 3]);
    for (index, ring) in patch.iter().enumerate() {
        let (a, m) = fan(ring, reference, normal, metric);
        let sign = if (index == 0) == (a >= 0.0_f64) { 1.0_f64 } else { -1.0_f64 };
        area += sign * a;
        moment = axpy(moment, sign, m);
    }
    let centroid = if area > 0.0_f64 { axpy(reference, 1.0 / area, moment) } else { reference };
    (area, centroid)
}

/// A ring's signed area along `normal` and first moment about `reference`.
fn fan(ring: &[Coord], reference: V3, normal: V3, metric: Metric) -> (f64, V3) {
    let (mut area, mut moment) = (Sum::default(), [0.0_f64; 3]);
    for pair in ring.windows(2) {
        if let [a, b] = pair {
            let (p, q) = (sub(lay(a.space(), metric), reference), sub(lay(b.space(), metric), reference));
            let piece = HALF * dot(cross(p, q), normal);
            area.add(piece);
            moment = axpy(moment, piece * THIRD, add(p, q));
        }
    }
    (area.value(), moment)
}

/// ST_Area or ST_3DArea of a patch surface (§8.1.2, §8.1.3); none when
/// it has no patches.
pub fn area(patches: &[Patch], metric: Metric) -> Option<f64> {
    if patches.is_empty() {
        return None;
    }
    let mut total = Sum::default();
    for patch in patches {
        total.add(patch_moments(patch, metric).0);
    }
    Some(total.value())
}

/// ST_Centroid or ST_3DCentroid of a patch surface (§8.1.6, §8.1.7):
/// the area-weighted centroid of its patches, or of its boundary when
/// it has no area.
pub fn centroid(patches: &[Patch], metric: Metric) -> Option<Coord> {
    if patches.is_empty() {
        return None;
    }
    let (mut weight, mut sum) = (0.0_f64, [0.0_f64; 3]);
    for patch in patches {
        let (a, c) = patch_moments(patch, metric);
        weight += a;
        sum = axpy(sum, a, c);
    }
    if weight > 0.0_f64 {
        return Some(Coord::at(scale(sum, 1.0 / weight), 0.0));
    }
    let (mut length, mut middle) = (0.0_f64, [0.0_f64; 3]);
    for (a, b) in boundary(patches) {
        let (p, q) = (lay(a, metric), lay(b, metric));
        let l = norm(sub(q, p));
        length += l;
        middle = axpy(middle, HALF * l, add(p, q));
    }
    let first = patches.first().and_then(|p| p.first()).and_then(|r| r.first()).map_or([0.0_f64; 3], |p| lay(p.space(), metric));
    Some(Coord::at(if length > 0.0 { scale(middle, 1.0 / length) } else { first }, 0.0))
}

/// An edge's key: its end points' bits, smaller first.
type EdgeKey = ([u64; 3], [u64; 3]);

fn key(a: V3, b: V3) -> EdgeKey {
    let (p, q) = (a.map(f64::to_bits), b.map(f64::to_bits));
    if p <= q { (p, q) } else { (q, p) }
}

/// The edges that exactly one patch ring uses, in first-use order.
pub fn boundary(patches: &[Patch]) -> Vec<(V3, V3)> {
    let mut uses: HashMap<EdgeKey, usize> = HashMap::new();
    let mut order = Vec::new();
    for ring in patches.iter().flatten() {
        for pair in ring.windows(2) {
            if let [a, b] = pair {
                let k = key(a.space(), b.space());
                let count = uses.entry(k).or_insert(0);
                if *count == 0 {
                    order.push((k, (a.space(), b.space())));
                }
                *count = count.saturating_add(1);
            }
        }
    }
    order.into_iter().filter(|(k, _)| uses.get(k) == Some(&1)).map(|(_, edge)| edge).collect()
}

/// ST_Perimeter or ST_3DPerimeter of a patch surface (§8.1.4, §8.1.5):
/// the length of its boundary.
pub fn perimeter(patches: &[Patch], metric: Metric) -> Option<f64> {
    if patches.is_empty() {
        return None;
    }
    let mut total = Sum::default();
    for (a, b) in boundary(patches) {
        total.add(metric.size(sub(b, a)));
    }
    Some(total.value())
}

/// ST_PointOnSurface or ST_3DPointOnSurf of a patch surface (§8.1.8,
/// §8.1.9): a point inside its largest patch.
///
/// # Errors
///
/// SQLSTATE 2FF74 for a patch ring that does not close.
pub fn point_on_surface(patches: &[Patch], metric: Metric) -> Result<Option<Coord>> {
    let largest = patches.iter().map(|p| (patch_moments(p, metric).0, p)).max_by(|(a, _), (b, _)| a.total_cmp(b));
    let Some((_, patch)) = largest else {
        return Ok(None);
    };
    let rings: Vec<Curve> = patch.iter().map(|r| Curve::LineString(r.clone())).collect();
    polygon_point(&rings, metric, &Planar)
}

/// Whether the patches form one piece, each reachable from the first
/// through edges they share (§8.5.1: a polyhedral surface's patches
/// meet along their boundaries).
pub fn is_contiguous(patches: &[Patch]) -> bool {
    let mut owner: HashMap<EdgeKey, usize> = HashMap::new();
    let mut parent: Vec<usize> = (0..patches.len()).collect();
    for (index, patch) in patches.iter().enumerate() {
        for pair in patch.iter().flat_map(|r| r.windows(2)) {
            if let [a, b] = pair {
                let k = key(a.space(), b.space());
                match owner.get(&k) {
                    Some(&other) => join(&mut parent, index, other),
                    None => {
                        owner.insert(k, index);
                    }
                }
            }
        }
    }
    let root = find(&parent, 0);
    (0..patches.len()).all(|i| find(&parent, i) == root)
}

fn find(parent: &[usize], mut i: usize) -> usize {
    for _ in 0..parent.len() {
        let next = parent.get(i).copied().unwrap_or(i);
        if next == i {
            break;
        }
        i = next;
    }
    i
}

fn join(parent: &mut [usize], a: usize, b: usize) {
    let (ra, rb) = (find(parent, a), find(parent, b));
    if let Some(slot) = parent.get_mut(ra.max(rb)) {
        *slot = ra.min(rb);
    }
}

#[cfg(test)]
mod tests;
