//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.1.2, §8.1.2, §8.1.4,
//! §10.3.5, §10.5.3, §10.5.5] The measures of values with arcs or other
//! curves. The planar engine (`routines`) takes a value's linear form, so
//! ST_Length, ST_Area and ST_Perimeter of a value with such a curve are
//! measured here, of the curves themselves, as is the perimeter of
//! polyhedral surfaces, TINs and compound surfaces.

use crate::error::Result;
use crate::geometry::model::Geom;
use crate::spatial::curves::geodesy::Planar;
use crate::spatial::curves::linearize::Tolerance;
use crate::spatial::curves::observe::{self, curve_to_line};
use crate::spatial::curves::patches::{self, Patch};
use crate::spatial::curves::piece::Metric;
use crate::spatial::curves::surface;
use crate::spatial::curves::types::Curve;
use crate::spatial::formats::model::from_geom;
use crate::spatial::formats::tree::{Body, Collection, Surface, Tin};

/// A measure of curves and surfaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exact {
    /// ST_Length of a curve or multicurve.
    Length,
    /// ST_Area of a surface or multisurface.
    Area,
    /// ST_Perimeter of a surface or multisurface.
    Perimeter,
}

/// A surface as the engines measure it.
enum Shape {
    /// A curve polygon's rings, exterior first.
    Rings(Vec<Curve>),
    /// Planar patches.
    Patches(Vec<Patch>),
}

/// The triangles of a TIN as patches.
fn triangles(tin: &Tin) -> Vec<Patch> {
    tin.triangles.iter().map(|t| vec![t.clone()]).collect()
}

/// The patches of `s`, its curved rings linearized.
fn flatten(s: &Surface, out: &mut Vec<Patch>) -> Result<()> {
    match s {
        Surface::CurvePolygon(rings) => {
            let linear = rings.iter().map(|c| curve_to_line(c, &Tolerance::DEFAULT, &Planar)).collect::<Result<Vec<_>>>()?;
            if !linear.is_empty() {
                out.push(linear);
            }
        }
        Surface::Polygon(rings) => out.extend((!rings.is_empty()).then(|| rings.clone())),
        Surface::Triangle(ring) => out.extend((!ring.is_empty()).then(|| vec![ring.clone()])),
        Surface::Polyhedral(p) => out.extend(p.iter().cloned()),
        Surface::Tin(t) => out.extend(triangles(t)),
        Surface::Compound(parts) => parts.iter().try_for_each(|p| flatten(p, out))?,
    }
    Ok(())
}

/// How the engines measure `s`.
fn shape(s: &Surface) -> Result<Shape> {
    Ok(match s {
        Surface::CurvePolygon(rings) => Shape::Rings(rings.clone()),
        Surface::Polygon(rings) => Shape::Rings(super::surfaces::lines(rings)),
        Surface::Triangle(ring) => {
            Shape::Rings(if ring.is_empty() { Vec::new() } else { super::surfaces::lines(std::slice::from_ref(ring)) })
        }
        Surface::Polyhedral(p) => Shape::Patches(p.clone()),
        Surface::Tin(t) => Shape::Patches(triangles(t)),
        Surface::Compound(_) => {
            let mut out = Vec::new();
            flatten(s, &mut out)?;
            Shape::Patches(out)
        }
    })
}

/// Whether `c` is other than a linestring.
const fn curved(c: &Curve) -> bool {
    !matches!(c, Curve::LineString(_))
}

/// Whether the measure of `s` is the curve engine's: a surface with a
/// ring other than a linestring, or the perimeter of patches, which is
/// their boundary's length and not the sum of their rings' [§8.1.4].
fn surface_curved(which: Exact, s: &Surface) -> bool {
    match s {
        Surface::CurvePolygon(rings) => rings.iter().any(curved),
        Surface::Compound(parts) => which == Exact::Perimeter || parts.iter().any(|p| surface_curved(which, p)),
        Surface::Polyhedral(_) | Surface::Tin(_) => which == Exact::Perimeter,
        Surface::Polygon(_) | Surface::Triangle(_) => false,
    }
}

/// The area or perimeter of the one surface `s`.
fn of_surface(which: Exact, s: &Surface) -> Result<Option<f64>> {
    let plane = Metric::Plane;
    Ok(match (which, shape(s)?) {
        (Exact::Area, Shape::Rings(r)) => surface::area(&r, plane, &Planar)?,
        (Exact::Perimeter, Shape::Rings(r)) => surface::perimeter(&r, plane, &Planar)?,
        (Exact::Area, Shape::Patches(p)) => patches::area(&p, plane),
        (Exact::Perimeter, Shape::Patches(p)) => patches::perimeter(&p, plane),
        (Exact::Length, Shape::Rings(_) | Shape::Patches(_)) => None,
    })
}

/// Adds `x` to the sum.
fn add(sum: &mut Option<f64>, x: Option<f64>) {
    if let Some(x) = x {
        *sum = Some(sum.unwrap_or(0.0_f64) + x);
    }
}

/// The measure of `g` by the curve engine, or `planar`'s if the planar
/// engine measures it exactly.
///
/// # Errors
///
/// A value the curve engine does not take.
pub fn exact(which: Exact, g: &Geom, planar: impl FnOnce() -> Option<f64>) -> Result<Option<f64>> {
    let tree = from_geom(g)?;
    let (curves, surfaces): (&[Curve], &[Surface]) = match &tree.body {
        Body::Curve(c) => (std::slice::from_ref(c), &[]),
        Body::Surface(s) => (&[], std::slice::from_ref(s)),
        Body::Collection(Collection::MultiCurve(c)) => (c, &[]),
        Body::Collection(Collection::MultiSurface(s)) => (&[], s),
        Body::Point(_)
        | Body::Solid(_)
        | Body::Collection(
            Collection::Geometry(_) | Collection::MultiPoint(_) | Collection::MultiLineString(_) | Collection::MultiPolygon(_),
        ) => return Ok(planar()),
    };
    if !curves.iter().any(curved) && !surfaces.iter().any(|s| surface_curved(which, s)) {
        return Ok(planar());
    }
    let mut sum = None;
    for c in curves {
        add(&mut sum, if which == Exact::Length { observe::length(c, Metric::Plane, &Planar)? } else { None });
    }
    for s in surfaces {
        add(&mut sum, of_surface(which, s)?);
    }
    Ok(sum)
}
