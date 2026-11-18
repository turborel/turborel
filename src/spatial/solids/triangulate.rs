//! \[DESIGN\] Planar patches in space cut into triangles, so the 3D
//! engine works on triangles alone. A patch is projected along its
//! normal's dominant axis, the dropped ordinate carried as the height,
//! and triangulated by the curve engine's constrained Delaunay mesh
//! (`curves::cdt`) with its rings' edges constrained. A triangle is kept
//! when its centroid lies inside the rings by the even-odd rule, so
//! holes need no orientation; kept triangles turn as the exterior ring.

use crate::error::Result;
use crate::spatial::curves::cdt::Mesh;

use super::{after, before, pick};

/// A point in space.
type P3 = [f64; 3];

/// The triangles covering the polygon of `rings` (exterior first, each
/// closed), each turning as the exterior ring does.
///
/// # Errors
///
/// As the mesh's constraints.
pub fn polygon(rings: &[Vec<P3>]) -> Result<Vec<[P3; 3]>> {
    let normal = newell(rings.first().map_or(&[][..], Vec::as_slice));
    let axis = dominant(normal);
    let project = |p: &P3| -> P3 { [pick(p, after(axis, 3)), pick(p, before(axis, 3)), pick(p, axis)] };
    let flat: Vec<Vec<P3>> = rings.iter().map(|r| r.iter().map(project).collect()).collect();
    let all = flat.iter().flatten();
    let low = all.clone().fold([f64::INFINITY; 2], |m, p| [m[0].min(p[0]), m[1].min(p[1])]);
    let high = all.fold([f64::NEG_INFINITY; 2], |m, p| [m[0].max(p[0]), m[1].max(p[1])]);
    if !low[0].is_finite() || !high[0].is_finite() {
        return Ok(Vec::new());
    }
    let mut mesh = Mesh::new(low, high);
    let mut edges = Vec::new();
    for ring in &flat {
        let ids: Vec<usize> = ring.iter().map(|p| mesh.insert(*p)).collect();
        edges.extend(ids.windows(2).filter_map(|w| if let [a, b] = w { Some((*a, *b)) } else { None }));
    }
    for (a, b) in edges {
        mesh.constrain(a, b)?;
    }
    let reversed = pick(&normal, axis) < 0.0_f64;
    let vertices = mesh.vertices();
    let at = |v: usize| vertices.get(v).copied().unwrap_or_default();
    let unproject = |p: P3| -> P3 {
        let mut out = [0.0_f64; 3];
        for (k, value) in [(after(axis, 3), p[0]), (before(axis, 3), p[1]), (axis, p[2])] {
            if let Some(slot) = out.get_mut(k) {
                *slot = value;
            }
        }
        out
    };
    Ok(mesh
        .triangles()
        .into_iter()
        .map(|t| t.map(at))
        .filter(|[a, b, c]| inside(&flat, [(a[0] + b[0] + c[0]) / 3.0_f64, (a[1] + b[1] + c[1]) / 3.0_f64]))
        .map(|t| {
            let [a, b, c] = t.map(unproject);
            if reversed { [a, c, b] } else { [a, b, c] }
        })
        .collect())
}

/// Whether `p` lies inside the projected `rings` by the even-odd rule.
fn inside(rings: &[Vec<P3>], p: [f64; 2]) -> bool {
    let mut odd = false;
    for ring in rings {
        for w in ring.windows(2) {
            let [a, b] = w else { continue };
            if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < a[0] + (p[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1]) {
                odd = !odd;
            }
        }
    }
    odd
}

/// The normal of a closed ring by Newell's method, its length twice the
/// area.
pub fn newell(ring: &[P3]) -> P3 {
    ring.windows(2).fold([0.0_f64; 3], |n, w| match w {
        [p, q] => [n[0] + (p[1] - q[1]) * (p[2] + q[2]), n[1] + (p[2] - q[2]) * (p[0] + q[0]), n[2] + (p[0] - q[0]) * (p[1] + q[1])],
        _ => n,
    })
}

/// The axis of the largest component of `normal`.
pub fn dominant(normal: P3) -> usize {
    let [x, y, z] = normal.map(f64::abs);
    if x >= y && x >= z {
        0
    } else if y >= z {
        1
    } else {
        2
    }
}
