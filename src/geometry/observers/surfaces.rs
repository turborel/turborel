//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8, §10.5, §5.1.13, §5.1.15]
//! The surface observers: the measures of every surface and multisurface
//! (§8.1, §10.5), the rings of curve polygons, polygons and triangles
//! (§8.2 to §8.4), the patches of polyhedral surfaces and TINs (§8.5,
//! §8.6), the parts of compound surfaces (§8.7) and the measure locators.

use super::{Observer, Parameter, Returns, Warned, array, double, found, int, number, part, whole};
use crate::error::{Result, SqlState};
use crate::geometry::model::{Geom, Kind};
use crate::geometry::representations::spatial;
use crate::spatial::curves::geodesy::Planar;
use crate::spatial::curves::linearize::Tolerance;
use crate::spatial::curves::observe::{self, curve_to_line};
use crate::spatial::curves::patches::{self, Patch};
use crate::spatial::curves::piece::Metric;
use crate::spatial::curves::types::{Coord, Curve};
use crate::spatial::curves::{interior, measure, surface};
use crate::spatial::formats::tree::{Body, Collection, Geometry, Ring, Surface, Tin};
use crate::value::Value;

/// A surface as the engines measure it.
enum Shape {
    /// A curve polygon's rings, exterior first.
    Rings(Vec<Curve>),
    /// Planar patches.
    Patches(Vec<Patch>),
}

/// The rings of a polygon as line strings.
fn lines(rings: &[Ring]) -> Vec<Curve> {
    rings.iter().cloned().map(Curve::LineString).collect()
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
        Surface::Polygon(rings) => Shape::Rings(lines(rings)),
        Surface::Triangle(ring) => Shape::Rings(if ring.is_empty() { Vec::new() } else { lines(std::slice::from_ref(ring)) }),
        Surface::Polyhedral(p) => Shape::Patches(p.clone()),
        Surface::Tin(t) => Shape::Patches(triangles(t)),
        Surface::Compound(_) => {
            let mut out = Vec::new();
            flatten(s, &mut out)?;
            Shape::Patches(out)
        }
    })
}

/// [§8.1, §10.5] The measures of every surface and multisurface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceMethod {
    /// [§8.1.2, §10.5.2] ST_Area().
    Area,
    /// [§8.1.4, §10.5.4] ST_Perimeter().
    Perimeter,
    /// [§8.1.6, §10.5.6] ST_Centroid().
    Centroid,
    /// [§8.1.8, §10.5.8] ST_PointOnSurface().
    PointOnSurface,
}

/// The measures of surfaces.
pub const SURFACE_METHODS: [SurfaceMethod; 4] =
    [SurfaceMethod::Area, SurfaceMethod::Perimeter, SurfaceMethod::Centroid, SurfaceMethod::PointOnSurface];

/// A measure of one surface: a number or a point.
enum Measured {
    Number(Option<f64>),
    Point(Option<Coord>),
}

impl SurfaceMethod {
    /// The measure of the one surface `s`.
    fn one(self, s: &Surface) -> Result<Measured> {
        let (geo, plane) = (&Planar, Metric::Plane);
        Ok(match (self, shape(s)?) {
            (Self::Area, Shape::Rings(r)) => Measured::Number(surface::area(&r, plane, geo)?),
            (Self::Perimeter, Shape::Rings(r)) => Measured::Number(surface::perimeter(&r, plane, geo)?),
            (Self::Centroid, Shape::Rings(r)) => Measured::Point(surface::centroid(&r, plane, geo)?),
            (Self::PointOnSurface, Shape::Rings(r)) => Measured::Point(interior::point_on_surface(&r, plane, geo)?),
            (Self::Area, Shape::Patches(p)) => Measured::Number(patches::area(&p, plane)),
            (Self::Perimeter, Shape::Patches(p)) => Measured::Number(patches::perimeter(&p, plane)),
            (Self::Centroid, Shape::Patches(p)) => Measured::Point(patches::centroid(&p, plane)),
            (Self::PointOnSurface, Shape::Patches(p)) => Measured::Point(patches::point_on_surface(&p, plane)?),
        })
    }

    /// The measure of the members `all` of a multisurface: the sum of
    /// their areas or perimeters, their centroid weighted by area, or the
    /// first member's point on it.
    fn many(self, all: &[Surface]) -> Result<Measured> {
        let mut sum: Option<f64> = None;
        let mut moment = Coord::default();
        for s in all {
            let area = match Self::Area.one(s)? {
                Measured::Number(a) => a,
                Measured::Point(_) => None,
            };
            match (self, self.one(s)?) {
                (Self::Area | Self::Perimeter, Measured::Number(Some(x))) => sum = Some(sum.unwrap_or(0.0_f64) + x),
                (Self::Centroid, Measured::Point(Some(c))) => {
                    let w = area.unwrap_or(0.0_f64);
                    sum = Some(sum.unwrap_or(0.0_f64) + w);
                    moment = Coord { x: c.x.mul_add(w, moment.x), y: c.y.mul_add(w, moment.y), z: c.z.mul_add(w, moment.z), m: 0.0 };
                }
                (Self::PointOnSurface, Measured::Point(Some(p))) => return Ok(Measured::Point(Some(p))),
                (Self::Area | Self::Perimeter | Self::Centroid | Self::PointOnSurface, Measured::Number(_) | Measured::Point(_)) => {}
            }
        }
        Ok(match self {
            Self::Area | Self::Perimeter => Measured::Number(sum),
            Self::Centroid => Measured::Point(sum.filter(|w| *w > 0.0_f64).map(|w| Coord {
                x: moment.x / w,
                y: moment.y / w,
                z: moment.z / w,
                m: 0.0_f64,
            })),
            Self::PointOnSurface => Measured::Point(None),
        })
    }
}

impl Observer for SurfaceMethod {
    fn name(self) -> &'static str {
        match self {
            Self::Area => "ST_AREA",
            Self::Perimeter => "ST_PERIMETER",
            Self::Centroid => "ST_CENTROID",
            Self::PointOnSurface => "ST_POINTONSURFACE",
        }
    }

    fn parameters(self) -> &'static [Parameter] {
        &[]
    }

    fn returns(self, _: Kind) -> Returns {
        match self {
            Self::Area | Self::Perimeter => Returns::Double,
            Self::Centroid | Self::PointOnSurface => Returns::Of(Kind::Point),
        }
    }

    fn warns(self) -> bool {
        false
    }

    fn of(self, g: &Geom, value: &Geometry, _: &[Value]) -> Result<Warned> {
        let measured = match &value.body {
            Body::Surface(s) => self.one(s)?,
            Body::Collection(Collection::MultiSurface(all)) => self.many(all)?,
            Body::Collection(Collection::MultiPolygon(all)) => self.many(&all.iter().cloned().map(Surface::Polygon).collect::<Vec<_>>())?,
            Body::Point(_) | Body::Curve(_) | Body::Solid(_) | Body::Collection(_) => {
                return Err(spatial(SqlState::SPATIAL_INVALID_ARGUMENT, "§8.1", &format!("{} takes a surface", self.name())));
            }
        };
        let result = match measured {
            Measured::Number(x) => double(x),
            Measured::Point(p) => p.map_or(Ok(Value::Null), |p| part(g, g.dims, Body::Point(Some(Coord { m: 0.0, ..p }))))?,
        };
        Ok((result, None))
    }
}

/// [§8.2 to §8.4] The rings of curve polygons, polygons and triangles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RingMethod {
    /// [§8.2.3, §8.3.3, §8.4.5] ST_ExteriorRing().
    ExteriorRing,
    /// [§8.2.4, §8.3.4, §8.4.6] ST_InteriorRings().
    InteriorRings,
    /// [§8.2.5] ST_NumInteriorRing().
    NumInteriorRing,
    /// [§8.2.6, §8.3.5, §8.4.7] ST_InteriorRingN(INTEGER).
    InteriorRingN,
    /// [§8.2.7] ST_CurvePolyToPoly().
    CurvePolyToPoly,
}

/// The methods of rings.
pub const RING_METHODS: [RingMethod; 5] = [
    RingMethod::ExteriorRing,
    RingMethod::InteriorRings,
    RingMethod::NumInteriorRing,
    RingMethod::InteriorRingN,
    RingMethod::CurvePolyToPoly,
];

/// The ring type of a surface type's rings: ST_LineString of polygons
/// and triangles, ST_Curve of curve polygons.
fn ring_kind(of: Kind) -> Kind {
    if of.is_subtype_of(Kind::Polygon) { Kind::LineString } else { Kind::Curve }
}

impl Observer for RingMethod {
    fn name(self) -> &'static str {
        match self {
            Self::ExteriorRing => "ST_EXTERIORRING",
            Self::InteriorRings => "ST_INTERIORRINGS",
            Self::NumInteriorRing => "ST_NUMINTERIORRING",
            Self::InteriorRingN => "ST_INTERIORRINGN",
            Self::CurvePolyToPoly => "ST_CURVEPOLYTOPOLY",
        }
    }

    fn parameters(self) -> &'static [Parameter] {
        if self == Self::InteriorRingN { &[Parameter::Integer] } else { &[] }
    }

    fn returns(self, of: Kind) -> Returns {
        match self {
            Self::ExteriorRing | Self::InteriorRingN => Returns::Of(ring_kind(of)),
            Self::InteriorRings => Returns::Array(ring_kind(of)),
            Self::NumInteriorRing => Returns::Integer,
            Self::CurvePolyToPoly => Returns::Of(Kind::Polygon),
        }
    }

    fn warns(self) -> bool {
        self == Self::InteriorRingN
    }

    fn of(self, g: &Geom, value: &Geometry, rest: &[Value]) -> Result<Warned> {
        let rings = match &value.body {
            Body::Surface(Surface::CurvePolygon(c)) => c.clone(),
            Body::Surface(Surface::Polygon(r)) => lines(r),
            Body::Surface(Surface::Triangle(r)) => {
                if r.is_empty() {
                    Vec::new()
                } else {
                    lines(std::slice::from_ref(r))
                }
            }
            Body::Point(_) | Body::Curve(_) | Body::Surface(_) | Body::Solid(_) | Body::Collection(_) => {
                return Err(spatial(SqlState::SPATIAL_INVALID_ARGUMENT, "§8.2", &format!("{} takes a polygon", self.name())));
            }
        };
        let Some((exterior, interiors)) = rings.split_first() else { return Ok((Value::Null, None)) };
        let ring = |c: Curve| part(g, g.dims, Body::Curve(c));
        let result = match self {
            Self::ExteriorRing => ring(exterior.clone())?,
            Self::InteriorRings => array(g, g.dims, interiors.iter().cloned().map(Body::Curve))?,
            Self::NumInteriorRing => int(interiors.len()),
            Self::InteriorRingN => return found(observe::nth(interiors, whole(rest.first(), self.name())?), ring),
            Self::CurvePolyToPoly => {
                let linear = rings.iter().map(|c| curve_to_line(c, &Tolerance::DEFAULT, &Planar)).collect::<Result<Vec<_>>>()?;
                part(g, g.dims, Body::Surface(Surface::Polygon(linear)))?
            }
        };
        Ok((result, None))
    }
}

/// [§8.5 to §8.7] The patches of polyhedral surfaces and TINs and the
/// parts of compound surfaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatchMethod {
    /// [§8.5.3, §8.6.7] ST_Patches().
    Patches,
    /// [§8.5.4] ST_NumPatches().
    NumPatches,
    /// [§8.5.5] ST_PatchN(INTEGER).
    PatchN,
    /// [§8.6.4] ST_MaxSideLength().
    MaxSideLength,
    /// [§8.7.3] ST_Surfaces().
    Surfaces,
    /// [§8.7.4] ST_NumSurfaces().
    NumSurfaces,
    /// [§8.7.5] ST_SurfaceN(INTEGER).
    SurfaceN,
}

/// The methods of patches and parts.
pub const PATCH_METHODS: [PatchMethod; 7] = [
    PatchMethod::Patches,
    PatchMethod::NumPatches,
    PatchMethod::PatchN,
    PatchMethod::MaxSideLength,
    PatchMethod::Surfaces,
    PatchMethod::NumSurfaces,
    PatchMethod::SurfaceN,
];

impl Observer for PatchMethod {
    fn name(self) -> &'static str {
        match self {
            Self::Patches => "ST_PATCHES",
            Self::NumPatches => "ST_NUMPATCHES",
            Self::PatchN => "ST_PATCHN",
            Self::MaxSideLength => "ST_MAXSIDELENGTH",
            Self::Surfaces => "ST_SURFACES",
            Self::NumSurfaces => "ST_NUMSURFACES",
            Self::SurfaceN => "ST_SURFACEN",
        }
    }

    fn parameters(self) -> &'static [Parameter] {
        if matches!(self, Self::PatchN | Self::SurfaceN) { &[Parameter::Integer] } else { &[] }
    }

    fn returns(self, of: Kind) -> Returns {
        let patch = if of == Kind::Tin { Kind::Triangle } else { Kind::Polygon };
        match self {
            Self::Patches => Returns::Array(patch),
            Self::PatchN => Returns::Of(patch),
            Self::NumPatches | Self::NumSurfaces => Returns::Integer,
            Self::MaxSideLength => Returns::Double,
            Self::Surfaces => Returns::Array(Kind::Surface),
            Self::SurfaceN => Returns::Of(Kind::Surface),
        }
    }

    fn warns(self) -> bool {
        matches!(self, Self::PatchN | Self::SurfaceN)
    }

    fn of(self, g: &Geom, value: &Geometry, rest: &[Value]) -> Result<Warned> {
        let parts: Vec<Surface> = match (&value.body, self) {
            (Body::Surface(Surface::Polyhedral(p)), Self::Patches | Self::NumPatches | Self::PatchN) => {
                p.iter().cloned().map(Surface::Polygon).collect()
            }
            (Body::Surface(Surface::Tin(t)), Self::Patches | Self::NumPatches | Self::PatchN) => {
                t.triangles.iter().cloned().map(Surface::Triangle).collect()
            }
            (Body::Surface(Surface::Tin(t)), Self::MaxSideLength) => return Ok((double(t.max_side), None)),
            (Body::Surface(Surface::Compound(c)), Self::Surfaces | Self::NumSurfaces | Self::SurfaceN) => c.clone(),
            (Body::Point(_) | Body::Curve(_) | Body::Surface(_) | Body::Solid(_) | Body::Collection(_), _) => {
                return Err(spatial(SqlState::SPATIAL_INVALID_ARGUMENT, "§8.5", &format!("{} takes no value of this type", self.name())));
            }
        };
        if parts.is_empty() {
            return Ok((Value::Null, None));
        }
        let one = |s: Surface| part(g, g.dims, Body::Surface(s));
        let result = match self {
            Self::Patches | Self::Surfaces | Self::MaxSideLength => array(g, g.dims, parts.into_iter().map(Body::Surface))?,
            Self::NumPatches | Self::NumSurfaces => int(parts.len()),
            Self::PatchN | Self::SurfaceN => return found(observe::nth(&parts, whole(rest.first(), self.name())?), one),
        };
        Ok((result, None))
    }
}

/// [§5.1.13, §5.1.15] The measure locators of every type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeasureMethod {
    /// [§5.1.13] ST_LocateAlong(DOUBLE PRECISION).
    LocateAlong,
    /// [§5.1.15] ST_LocateBetween(DOUBLE PRECISION, DOUBLE PRECISION).
    LocateBetween,
}

/// The measure locators.
pub const MEASURE_METHODS: [MeasureMethod; 2] = [MeasureMethod::LocateAlong, MeasureMethod::LocateBetween];

impl Observer for MeasureMethod {
    fn name(self) -> &'static str {
        match self {
            Self::LocateAlong => "ST_LOCATEALONG",
            Self::LocateBetween => "ST_LOCATEBETWEEN",
        }
    }

    fn parameters(self) -> &'static [Parameter] {
        match self {
            Self::LocateAlong => &[Parameter::Double],
            Self::LocateBetween => &[Parameter::Double, Parameter::Double],
        }
    }

    fn returns(self, _: Kind) -> Returns {
        Returns::Of(Kind::Geometry)
    }

    fn warns(self) -> bool {
        false
    }

    fn of(self, g: &Geom, value: &Geometry, rest: &[Value]) -> Result<Warned> {
        let start = number(rest.first(), self.name())?;
        let end = if self == Self::LocateAlong { start } else { number(rest.get(1), self.name())? };
        if end < start {
            return Err(spatial(SqlState::SPATIAL_INVALID_ARGUMENT, "§5.1.15", "the end measure is less than the start measure"));
        }
        if g.is_empty() {
            return Ok((Value::Null, None));
        }
        let located = measure::locate_between(value, (start, end), &Planar)?;
        Ok((part(g, located.dims, located.body)?, None))
    }
}
