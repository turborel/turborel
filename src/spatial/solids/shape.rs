//! \[DESIGN\] The 3D boundary, closedness and simplicity of ISO/IEC
//! 13249-3:2016 (DIS, SC32 N2593) §5.1.17, §5.1.18, §7.1.7, §8.1.11 and
//! §8.1.12. A curve's boundary is its ends unless they meet in space, a
//! multicurve's the ends an odd number of its curves have. A polygon's is
//! its rings; a patch surface's the edges exactly one patch ring uses; a
//! solid's its shells. A surface is closed when that boundary is empty,
//! and simple when its patches meet only along their own boundaries,
//! which the exact engine tests patch against patch.

use std::collections::HashMap;

use super::assemble::chain;
use super::cells::{Cells, P3, Pieces, patches};
use super::contact::{Member, Operand, Reach, polys_meet, split_in};
use super::exact::Scale;
use super::pieces::{Poly, Seg};
use crate::error::Result;
use crate::geometry::model::{Body, Collection, Curve, Dims, Geom, Surface};
use crate::spatial::curves::adapt::from_model;
use crate::spatial::curves::geodesy::Planar;
use crate::spatial::curves::observe;
use crate::spatial::curves::patches::boundary as patch_boundary;
use crate::spatial::curves::piece::Metric;
use crate::spatial::curves::types::Coord;

/// §5.1.18 ST_3DBoundary: the closure of `g`'s boundary in space; None
/// for an empty value.
///
/// # Errors
///
/// As the curve engine's ends of a curve.
pub fn boundary(g: &Geom) -> Result<Option<Geom>> {
    if g.is_empty() {
        return Ok(None);
    }
    let body = of_body(&g.body, g.dims)?;
    let dims = if matches!(body, Bound::Edges(_)) { flat_dims(g.dims) } else { g.dims };
    let made = match body {
        Bound::Body(b) => b,
        Bound::Edges(edges) => lines(chain(&edges), dims),
    };
    Geom::new(g.srid, dims, made).map(Some)
}

/// A boundary: a body of the value's dimensions, or edges in space.
enum Bound {
    Body(Body),
    Edges(Vec<[P3; 2]>),
}

/// The dimensions of edges taken in space: x, y and z if `dims` has z.
fn flat_dims(dims: Dims) -> Dims {
    if dims.z_at().is_some() { Dims::Xyz } else { Dims::Xy }
}

fn empty() -> Body {
    Body::Collection(Collection::Geometry(Vec::new()))
}

/// The boundary of one body.
fn of_body(body: &Body, dims: Dims) -> Result<Bound> {
    Ok(match body {
        Body::Point(_) => Bound::Body(empty()),
        Body::Curve(c) => Bound::Body(ends(std::slice::from_ref(c), dims)?),
        Body::Surface(s) => surface(s, dims),
        Body::Solid(shells) => Bound::Body(match shells.as_slice() {
            [one] => Body::Surface(one.clone()),
            many => Body::Collection(Collection::MultiSurface(many.to_vec())),
        }),
        Body::Collection(c) => collection(c, dims)?,
    })
}

fn collection(c: &Collection, dims: Dims) -> Result<Bound> {
    Ok(match c {
        Collection::MultiPoint(_) => Bound::Body(empty()),
        Collection::MultiCurve(curves) => Bound::Body(ends(curves, dims)?),
        Collection::MultiLineString(lines) => Bound::Body(ends(&lines.iter().cloned().map(Curve::LineString).collect::<Vec<_>>(), dims)?),
        Collection::MultiSurface(surfaces) => Bound::Edges(edges(surfaces.iter(), dims)),
        Collection::MultiPolygon(polygons) => {
            Bound::Edges(edges(polygons.iter().map(|r| Surface::Polygon(r.clone())).collect::<Vec<_>>().iter(), dims))
        }
        Collection::Geometry(parts) => {
            let mut out = Vec::new();
            for part in parts {
                out.push(match of_body(part, dims)? {
                    Bound::Body(b) => b,
                    Bound::Edges(e) => lines(chain(&e), dims),
                });
            }
            Bound::Body(Body::Collection(Collection::Geometry(out)))
        }
    })
}

/// A polygon's rings, or a patch surface's edges one patch uses.
fn surface(s: &Surface, dims: Dims) -> Bound {
    match s {
        Surface::Polygon(rings) => Bound::Body(match rings.as_slice() {
            [one] => Body::Curve(Curve::LineString(one.clone())),
            many => Body::Collection(Collection::MultiLineString(many.to_vec())),
        }),
        Surface::CurvePolygon(rings) => Bound::Body(match rings.as_slice() {
            [one] => Body::Curve(one.clone()),
            many => Body::Collection(Collection::MultiCurve(many.to_vec())),
        }),
        Surface::Triangle(_) | Surface::PolyhdrlSurface(_) | Surface::Tin(_) | Surface::Compound(_) => {
            Bound::Edges(edges(std::iter::once(s), dims))
        }
    }
}

/// The edges exactly one patch ring of `surfaces` uses.
pub fn edges<'s>(surfaces: impl Iterator<Item = &'s Surface>, dims: Dims) -> Vec<[P3; 2]> {
    let all: Vec<Vec<Vec<Coord>>> = surfaces
        .flat_map(|s| patches(s, dims))
        .map(|p| p.into_iter().map(|r| r.into_iter().map(|q| Coord::at(q, 0.0_f64)).collect()).collect())
        .collect();
    patch_boundary(&all).into_iter().map(|(a, b)| [a, b]).collect()
}

/// Line strings of `chains` in `dims`, x and y or x, y and z.
fn lines(chains: Vec<Vec<P3>>, dims: Dims) -> Body {
    let flat =
        |c: Vec<P3>| -> Vec<f64> { c.into_iter().flat_map(|p| if dims == Dims::Xyz { p.to_vec() } else { vec![p[0], p[1]] }).collect() };
    let mut all: Vec<Vec<f64>> = chains.into_iter().map(flat).collect();
    match all.len() {
        0 => empty(),
        1 => Body::Curve(Curve::LineString(all.pop().unwrap_or_default())),
        _ => Body::Collection(Collection::MultiLineString(all)),
    }
}

/// The ends that an odd number of `curves` have and that do not meet
/// their own curve's other end in space, as points of `dims`.
fn ends(curves: &[Curve], dims: Dims) -> Result<Body> {
    let mut count: HashMap<[u64; 3], (usize, Vec<f64>)> = HashMap::new();
    let mut order = Vec::new();
    for curve in curves {
        let engine = from_model(curve, dims);
        if engine.is_empty() || observe::is_closed(&engine, Metric::Space, &Planar)? {
            continue;
        }
        for end in [observe::start_point(&engine, &Planar)?, observe::end_point(&engine, &Planar)?].into_iter().flatten() {
            let key = end.space().map(f64::to_bits);
            let entry = count.entry(key).or_insert_with(|| {
                order.push(key);
                (0, coords_of(end, dims))
            });
            entry.0 = entry.0.saturating_add(1);
        }
    }
    let points: Vec<Vec<f64>> =
        order.iter().filter_map(|k| count.get(k)).filter(|(n, _)| !n.is_multiple_of(2)).map(|(_, p)| p.clone()).collect();
    Ok(if points.is_empty() { empty() } else { Body::Collection(Collection::MultiPoint(points)) })
}

/// The numbers of `c` in `dims`.
fn coords_of(c: Coord, dims: Dims) -> Vec<f64> {
    let mut out = vec![c.x, c.y];
    if dims.z_at().is_some() {
        out.push(c.z);
    }
    if dims.m_at().is_some() {
        out.push(c.m);
    }
    out
}

/// §8.1.11 ST_3DIsClosed of a surface: not empty, and no edge used by
/// exactly one patch ring.
pub fn surface_is_closed(g: &Geom) -> bool {
    let Body::Surface(s) = &g.body else { return false };
    !g.is_empty() && edges(std::iter::once(s), g.dims).is_empty()
}

/// §5.1.17 ST_3DIsSimple of the patches of surfaces and solids: no two
/// patches meet but along both their boundaries.
///
/// # Errors
///
/// As [`Pieces`]' triangulation.
pub fn patches_simple(surfaces: &[&Surface], dims: Dims) -> Result<bool> {
    let all: Vec<Vec<Vec<P3>>> = surfaces.iter().flat_map(|s| patches(s, dims)).collect();
    let numbers = all.iter().flatten().flatten().flatten().copied();
    let scale = Scale::of(numbers);
    let mut prepared = Vec::with_capacity(all.len());
    for patch in &all {
        let cells = Cells::of_patch(patch)?;
        let exact = cells.exact(scale);
        let rim: Vec<Seg> = patch
            .iter()
            .flat_map(|r| r.windows(2))
            .filter_map(|w| match w {
                [a, b] => Seg::of(&a.map(|v| scale.integer(v)), &b.map(|v| scale.integer(v))),
                _ => None,
            })
            .collect();
        prepared.push((exact.polys, Operand::new(Pieces { segments: rim, ..Pieces::default() })));
    }
    for (i, (first, first_rim)) in prepared.iter().enumerate() {
        for (second, second_rim) in prepared.iter().skip(i.saturating_add(1)) {
            if !meet_on_rims(first, second, first_rim, second_rim) {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// Whether the triangles of two patches meet only on both rims.
fn meet_on_rims(first: &[Poly], second: &[Poly], first_rim: &Operand, second_rim: &Operand) -> bool {
    for a in first {
        for b in second {
            if !a.clone().in_poly(b).0.is_empty() {
                return false;
            }
            let (segments, points) = polys_meet(a, b);
            let off_rim = |rim: &Operand| {
                segments.iter().any(|s| !split_in(s.clone(), rim, Reach::Curves).1.is_empty())
                    || points.iter().any(|p| !split_in(p.clone(), rim, Reach::Curves).1.is_empty())
            };
            if off_rim(first_rim) || off_rim(second_rim) {
                return false;
            }
        }
    }
    true
}
