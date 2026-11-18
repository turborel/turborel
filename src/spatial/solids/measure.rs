//! \[DESIGN\] The measures of solids, ISO/IEC 13249-3:2016 (DIS, SC32
//! N2593) §9.1.2 to §9.1.5, and the slope of a triangle, §8.4.4. A
//! solid's shells are triangulated and turned outward from the material
//! ([`super::cells`]); its volume is the divergence theorem's sum over
//! tetrahedra from one point, its centroid their weighted centres, and
//! its surface area the triangles' areas.

use super::cells::{Cells, P3, sub};
use super::contact::Operand;
use super::exact::Scale;
use super::kernel::Hom;
use crate::error::Result;
use crate::geometry::model::Geom;

/// A solid's triangles, outward, every shell together.
fn triangles(g: &Geom) -> Result<Vec<[P3; 3]>> {
    Ok(Cells::of(g)?.solids.into_iter().flatten().collect())
}

fn cross(u: P3, v: P3) -> P3 {
    [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]]
}

fn norm(v: P3) -> f64 {
    v[0].hypot(v[1]).hypot(v[2])
}

/// The signed volume of the tetrahedron of `o` and triangle `t`.
fn tetrahedron(o: P3, [a, b, c]: [P3; 3]) -> f64 {
    super::cells::triple(sub(a, o), sub(b, o), sub(c, o)) / 6.0_f64
}

/// §9.1.3 ST_3DVolume: the volume of the solid `g`.
///
/// # Errors
///
/// As [`Cells::of`].
pub fn volume(g: &Geom) -> Result<f64> {
    let all = triangles(g)?;
    let origin = all.first().map_or([0.0_f64; 3], |t| t[0]);
    Ok(all.iter().map(|t| tetrahedron(origin, *t)).sum::<f64>().abs())
}

/// §9.1.2 ST_3DSurfaceArea: the area of the solid `g`'s boundary.
///
/// # Errors
///
/// As [`Cells::of`].
pub fn surface_area(g: &Geom) -> Result<f64> {
    Ok(triangles(g)?.iter().map(|[a, b, c]| 0.5_f64 * norm(cross(sub(*b, *a), sub(*c, *a)))).sum())
}

/// §9.1.4 ST_3DCentroid: the centroid of the solid `g`'s volume; None if
/// it has none.
///
/// # Errors
///
/// As [`Cells::of`].
pub fn centroid(g: &Geom) -> Result<Option<P3>> {
    let all = triangles(g)?;
    let Some(origin) = all.first().map(|t| t[0]) else { return Ok(None) };
    let (mut weight, mut sum) = (0.0_f64, [0.0_f64; 3]);
    for t in &all {
        let v = tetrahedron(origin, *t);
        weight += v;
        for (k, slot) in sum.iter_mut().enumerate() {
            let coordinate = |p: &P3| p.get(k).copied().unwrap_or_default();
            *slot += v * (coordinate(&origin) + coordinate(&t[0]) + coordinate(&t[1]) + coordinate(&t[2])) / 4.0_f64;
        }
    }
    Ok((weight != 0.0_f64).then(|| sum.map(|s| s / weight)))
}

/// §9.1.5 ST_3DPointOnSolid: a point in the solid `g`: its centroid if
/// that lies in it, else the middle of the segment from the centroid of
/// a face's triangle a little way in, else a vertex of its boundary.
///
/// # Errors
///
/// As [`Cells::of`].
pub fn point_on(g: &Geom) -> Result<Option<P3>> {
    let cells = Cells::of(g)?;
    let all: Vec<[P3; 3]> = cells.solids.iter().flatten().copied().collect();
    let Some(vertex) = all.first().map(|t| t[0]) else { return Ok(None) };
    let mut candidates: Vec<P3> = centroid(g)?.into_iter().collect();
    candidates.extend(all.iter().take(INWARD_TRIES).filter_map(inward));
    let scale = Scale::of(cells.numbers().chain(candidates.iter().flatten().copied()));
    let solid = Operand::new(cells.exact(scale));
    let inside = |p: &P3| {
        let h = Hom::of(p.map(|v| scale.integer(v)));
        solid.against(h).iter().all(|(_, label)| *label == super::bsp::Label::In)
    };
    Ok(Some(candidates.into_iter().find(|p| p.iter().all(|v| v.is_finite()) && inside(p)).unwrap_or(vertex)))
}

/// How many faces' points within are tried.
const INWARD_TRIES: usize = 16;

/// A point just inside the face triangle `t`, which faces out: its
/// centroid moved a hundredth of its size against its normal.
fn inward([a, b, c]: &[P3; 3]) -> Option<P3> {
    let normal = cross(sub(*b, *a), sub(*c, *a));
    let size = norm(normal);
    if size <= 0.0_f64 {
        return None;
    }
    let step = size.sqrt() / 100.0_f64 / size;
    let middle = [(a[0] + b[0] + c[0]) / 3.0_f64, (a[1] + b[1] + c[1]) / 3.0_f64, (a[2] + b[2] + c[2]) / 3.0_f64];
    Some([middle[0] - normal[0] * step, middle[1] - normal[1] * step, middle[2] - normal[2] * step])
}

/// §8.4.4 ST_3DSlope: the slope of the triangle `a`, `b`, `c` as rise
/// over run, the tangent of its plane's angle to the x-y plane; None for
/// a vertical or degenerate triangle.
pub fn slope([a, b, c]: [P3; 3]) -> Option<f64> {
    let n = cross(sub(b, a), sub(c, a));
    let run = n[2].abs();
    (run > 0.0_f64).then(|| n[0].hypot(n[1]) / run)
}
