//! \[DESIGN\] Walks over a geometry value's parts the model shares: each
//! number of it, a curve's end points where its points give them, a ring's
//! closure and a compound curve's contiguity [ISO/IEC 13249-3:2016 (DIS,
//! SC32 N2593) §4.2.4, §7.10], and a NURBS curve's structure [§4.2.10].

use super::check::{MIN_RING, failed, invalid, limited};
use super::{Body, Collection, Coords, Curve, Dims, Measures, Nurbs, Placement, Surface};
use crate::error::{Result, SqlState};
use crate::geometry::MAX_DEPTH;

/// Applies `f` to every number of `body`: ordinates, placements, weights,
/// knots and parameters. A body past `MAX_DEPTH` was refused by `check`
/// before this runs, and its deeper parts are left as they are.
pub(super) fn numbers_mut(body: &mut Body, f: &mut impl FnMut(&mut f64)) {
    body_numbers(body, f, 1);
}

fn body_numbers(body: &mut Body, f: &mut impl FnMut(&mut f64), depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    let deeper = depth.saturating_add(1);
    match body {
        Body::Point(c) => c.iter_mut().for_each(&mut *f),
        Body::Curve(c) => curve_numbers(c, f, deeper),
        Body::Surface(s) => surface_numbers(s, f, deeper),
        Body::Solid(shells) => shells.iter_mut().for_each(|s| surface_numbers(s, f, deeper)),
        Body::Collection(c) => match c {
            Collection::Geometry(parts) => parts.iter_mut().for_each(|p| body_numbers(p, f, deeper)),
            Collection::MultiPoint(points) | Collection::MultiLineString(points) => points.iter_mut().flatten().for_each(&mut *f),
            Collection::MultiCurve(curves) => curves.iter_mut().for_each(|one| curve_numbers(one, f, deeper)),
            Collection::MultiSurface(surfaces) => surfaces.iter_mut().for_each(|s| surface_numbers(s, f, deeper)),
            Collection::MultiPolygon(polygons) => polygons.iter_mut().flatten().flatten().for_each(&mut *f),
        },
    }
}

fn curve_numbers(c: &mut Curve, f: &mut impl FnMut(&mut f64), depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    match c {
        Curve::LineString(p) | Curve::CircularString(p) | Curve::Circle(p) | Curve::GeodesicString(p) => p.iter_mut().for_each(&mut *f),
        Curve::Elliptical(Some(e)) => {
            placement_numbers(&mut e.placement, f);
            [&mut e.u_axis, &mut e.v_axis, &mut e.start_angle, &mut e.end_angle].into_iter().for_each(&mut *f);
            measure_numbers(e.measures.as_mut(), f);
        }
        Curve::Clothoid(Some(c)) => {
            placement_numbers(&mut c.placement, f);
            [&mut c.scale_factor, &mut c.start_distance, &mut c.end_distance].into_iter().for_each(&mut *f);
            measure_numbers(c.measures.as_mut(), f);
        }
        Curve::Spiral(Some(s)) => {
            placement_numbers(&mut s.placement, f);
            [&mut s.length, &mut s.start_curvature, &mut s.end_curvature].into_iter().for_each(&mut *f);
            measure_numbers(s.measures.as_mut(), f);
        }
        Curve::Nurbs(Some(n)) => {
            for p in &mut n.points {
                p.point.iter_mut().chain(p.weight.as_mut()).for_each(&mut *f);
            }
            n.knots.iter_mut().for_each(|k| f(&mut k.value));
            measure_numbers(n.measures.as_mut(), f);
        }
        Curve::Elliptical(None) | Curve::Clothoid(None) | Curve::Spiral(None) | Curve::Nurbs(None) => {}
        Curve::Compound(parts) => parts.iter_mut().for_each(|p| curve_numbers(p, f, depth.saturating_add(1))),
    }
}

fn placement_numbers(p: &mut Placement, f: &mut impl FnMut(&mut f64)) {
    p.location.iter_mut().chain(p.directions.iter_mut()).for_each(f);
}

fn measure_numbers(m: Option<&mut Measures>, f: &mut impl FnMut(&mut f64)) {
    if let Some(m) = m {
        f(&mut m.start);
        f(&mut m.end);
    }
}

fn surface_numbers(s: &mut Surface, f: &mut impl FnMut(&mut f64), depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    match s {
        Surface::CurvePolygon(rings) => rings.iter_mut().for_each(|r| curve_numbers(r, f, depth.saturating_add(1))),
        Surface::Polygon(rings) => rings.iter_mut().flatten().for_each(&mut *f),
        Surface::Triangle(ring) => ring.iter_mut().for_each(&mut *f),
        Surface::PolyhdrlSurface(patches) => patches.iter_mut().flatten().flatten().for_each(&mut *f),
        Surface::Tin(tin) => {
            tin.triangles.iter_mut().flatten().for_each(&mut *f);
            tin.max_side_length.iter_mut().for_each(&mut *f);
            tin.elements.iter_mut().for_each(|e| body_numbers(&mut e.geometry, f, depth.saturating_add(1)));
        }
        Surface::Compound(parts) => parts.iter_mut().for_each(|p| surface_numbers(p, f, depth.saturating_add(1))),
    }
}

/// [§4.2.4] A curve's start and end points, of the ordinates a point is
/// compared by (`same_point`), where its points give them: a curve of
/// points, a circle, which ends where it starts, and a compound curve of
/// such curves at its ends. None of an empty curve, or one given by
/// parameters, whose ends are computed (`geometry/curve`).
pub fn end_points(c: &Curve, dims: Dims) -> Option<(&[f64], &[f64])> {
    let stride = dims.stride();
    let mut at = c;
    let mut last = c;
    for _ in 0..MAX_DEPTH {
        let Curve::Compound(parts) = at else { break };
        at = parts.first()?;
    }
    for _ in 0..MAX_DEPTH {
        let Curve::Compound(parts) = last else { break };
        last = parts.last()?;
    }
    let start = first_point(at)?.get(..stride)?;
    let end = match last {
        Curve::Circle(p) => p.get(..stride)?,
        Curve::LineString(p) | Curve::CircularString(p) | Curve::GeodesicString(p) => p.get(p.len().checked_sub(stride)?..)?,
        Curve::Elliptical(_) | Curve::Nurbs(_) | Curve::Clothoid(_) | Curve::Spiral(_) | Curve::Compound(_) => return None,
    };
    Some((start, end))
}

fn first_point(c: &Curve) -> Option<&Coords> {
    match c {
        Curve::LineString(p) | Curve::CircularString(p) | Curve::Circle(p) | Curve::GeodesicString(p) => Some(p),
        Curve::Elliptical(_) | Curve::Nurbs(_) | Curve::Clothoid(_) | Curve::Spiral(_) | Curve::Compound(_) => None,
    }
}

/// [§4.2.4] Whether two points are equal: of x, y and z, as a curve's end
/// and the next one's start are, m not compared [§4.2.1.6 rule 3].
pub fn same_point(a: &[f64], b: &[f64], dims: Dims) -> bool {
    let n = dims.planar();
    match (a.get(..n), b.get(..n)) {
        (Some(a), Some(b)) => a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits()),
        (None, _) | (_, None) => false,
    }
}

/// [§8.3] That `ring`, the points of a polygon's ring, has at least four
/// points, its last its first.
pub(super) fn ring(ring: &Coords, dims: Dims) -> Result<()> {
    let stride = dims.stride();
    if ring.len() < MIN_RING.saturating_mul(stride) {
        return Err(invalid("§8.3", "a polygon's ring has at least four points"));
    }
    let closed = ring.len().checked_sub(stride).and_then(|at| ring.get(at..)).is_some_and(|last| same_point(ring, last, dims));
    if !closed {
        return Err(invalid("§8.3", "a polygon's ring ends at its first point"));
    }
    Ok(())
}

/// [§8.2] That a curve polygon's ring is not empty and, where its points
/// give its ends, closed.
pub(super) fn closed_ring(ring: &Curve, dims: Dims) -> Result<()> {
    if ring.is_empty() {
        return Err(failed(SqlState::SPATIAL_NULL_EXTERIOR_RING, "§8.2", "a curve polygon's rings are not empty"));
    }
    if let Some((start, end)) = end_points(ring, dims)
        && !same_point(start, end, dims)
    {
        return Err(invalid("§8.2", "a curve polygon's ring ends where it starts"));
    }
    Ok(())
}

/// [§7.10] That each curve of a compound curve ends where the next starts,
/// where their points give their ends.
pub(super) fn contiguous(parts: &[Curve], dims: Dims) -> Result<()> {
    for pair in parts.windows(2) {
        let [a, b] = pair else { continue };
        if let (Some((_, end)), Some((start, _))) = (end_points(a, dims), end_points(b, dims))
            && !same_point(end, start, dims)
        {
            return Err(failed(SqlState::SPATIAL_NON_CONTIGUOUS_CURVES, "§7.10", "a compound curve's curves meet end to start"));
        }
    }
    Ok(())
}

/// [§4.2.10, §17.4, §17.5] That a NURBS curve has control points, a
/// degree of at least 1, control points of `planar` finite ordinates, all
/// weighted and positive or none, knots strictly increasing of
/// multiplicity at least 1, whose multiplicities sum to its points and its
/// degree and one more [ISO 19107 B-spline].
pub(super) fn nurbs(n: &Nurbs, planar: usize) -> Result<()> {
    if n.points.is_empty() {
        return Err(invalid("§7.7", "a NURBS curve has control points; an empty one is none"));
    }
    let weighted = n.points.iter().filter(|p| p.weight.is_some()).count();
    let well_formed = n.points.iter().all(|p| p.point.len() == planar && p.weight.is_none_or(|w| w > 0.0_f64));
    if n.degree == 0 || !well_formed || (weighted != 0 && weighted != n.points.len()) {
        return Err(invalid("§7.7", "a NURBS curve has a degree, control points of its dimensions, and every weight positive or none"));
    }
    let increasing = n.knots.windows(2).all(|k| k.first().zip(k.get(1)).is_some_and(|(a, b)| a.value < b.value));
    let multiplicities =
        n.knots.iter().try_fold(0_usize, |sum, k| (k.multiplicity >= 1).then(|| sum.saturating_add(usize::from(k.multiplicity))));
    let wanted = n.points.len().saturating_add(usize::from(n.degree)).saturating_add(1);
    if !increasing || multiplicities != Some(wanted) {
        return Err(invalid(
            "§7.7",
            "a NURBS curve's knots increase, and their multiplicities sum to its control points and its degree and one",
        ));
    }
    limited(n.points.len(), crate::geometry::MAX_PARTS)?;
    for p in &n.points {
        super::check::finite(&p.point)?;
        super::check::finite(&p.weight.into_iter().collect::<Vec<_>>())?;
    }
    super::check::finite(&n.knots.iter().map(|k| k.value).collect::<Vec<_>>())
}

/// [§5.1.67 `<letters>`] Whether `text` is of letters: simple Latin
/// letters, digits, and the specials ( ) - _ . ' and space.
pub(super) fn letters(text: &str) -> bool {
    /// The specials of `<special>`.
    const SPECIALS: &str = "()-_.' ";
    text.chars().all(|c| c.is_ascii_alphanumeric() || SPECIALS.contains(c))
}
