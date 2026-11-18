//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.2.1.5, Tables 2 to 4] A
//! value cast to another type of the hierarchy: to a supertype of its
//! own, as it is; an empty set, to the empty value of any instantiable
//! type (code E); else as Table 4 lets it. A curve or a surface is wrapped
//! in the compound or the collection of one element, and a collection of
//! one element, or a compound of one, is unwrapped to it and cast by its
//! own row (1-DT, and 1-DT1 1-DT2 read so). A curve cast to a linestring
//! is linearized (`Geom::linear`), a polygon of one ring of four points
//! cast to a triangle is that triangle (1-LS 4-P), and a collection of
//! several elements, each of the elements' type, casts to the multi type
//! of them (n-DT).

use super::check::{MIN_RING, failed, limited};
use super::{Body, Collection, Curve, Geom, Kind, Rings, Surface, Tin};
use crate::error::{Result, SqlState};
use crate::geometry::MAX_DEPTH;

/// [§4.2.1.5] `g` cast to `target`.
///
/// # Errors
///
/// SQLSTATE 2FF08 (element is not a valid type) if Table 4 does not take
/// it; as `Geom::linear` of a curve no linearizer takes.
pub fn cast(g: &Geom, target: Kind) -> Result<Geom> {
    cast_at(g, target, 1)
}

fn refused(g: &Geom, target: Kind) -> crate::error::SqlError {
    failed(
        SqlState::SPATIAL_ELEMENT_IS_NOT_A_VALID_TYPE,
        "§4.2.1.5 Table 4",
        &format!("a value of {} is not cast to {}", g.kind().sql_name(), target.sql_name()),
    )
}

fn cast_at(g: &Geom, target: Kind, depth: usize) -> Result<Geom> {
    limited(depth, MAX_DEPTH)?;
    let source = g.kind();
    if source.is_subtype_of(target) {
        return Ok(g.clone());
    }
    if g.is_empty() {
        return empty(target).map(|b| g.part(b)).ok_or_else(|| refused(g, target));
    }
    if let Some(body) = converted(g, target)? {
        return Geom::new(g.srid, g.dims, body);
    }
    let one = only_element(&g.body).filter(|_| !is_wrapper(target)).ok_or_else(|| refused(g, target))?;
    cast_at(&g.part(one), target, depth.saturating_add(1))
}

/// Whether casting to `target` wraps a value rather than unwrapping it.
fn is_wrapper(target: Kind) -> bool {
    matches!(
        target,
        Kind::GeomCollection | Kind::MultiPoint | Kind::MultiCurve | Kind::MultiLineString | Kind::MultiSurface | Kind::MultiPolygon
    )
}

/// [Table 3 E] The empty value of `target`, none of a type not
/// instantiable.
pub(crate) fn empty(target: Kind) -> Option<Body> {
    Some(match target {
        Kind::Point => Body::Point(Vec::new()),
        Kind::LineString => Body::Curve(Curve::LineString(Vec::new())),
        Kind::CircularString => Body::Curve(Curve::CircularString(Vec::new())),
        Kind::Circle => Body::Curve(Curve::Circle(Vec::new())),
        Kind::GeodesicString => Body::Curve(Curve::GeodesicString(Vec::new())),
        Kind::EllipticalCurve => Body::Curve(Curve::Elliptical(None)),
        Kind::NurbsCurve => Body::Curve(Curve::Nurbs(None)),
        Kind::Clothoid => Body::Curve(Curve::Clothoid(None)),
        Kind::SpiralCurve => Body::Curve(Curve::Spiral(None)),
        Kind::CompoundCurve => Body::Curve(Curve::Compound(Vec::new())),
        Kind::CurvePolygon => Body::Surface(Surface::CurvePolygon(Vec::new())),
        Kind::Polygon => Body::Surface(Surface::Polygon(Vec::new())),
        Kind::Triangle => Body::Surface(Surface::Triangle(Vec::new())),
        Kind::PolyhdrlSurface => Body::Surface(Surface::PolyhdrlSurface(Vec::new())),
        Kind::Tin => Body::Surface(Surface::Tin(Tin { triangles: Vec::new(), elements: Vec::new(), max_side_length: None })),
        Kind::CompoundSurface => Body::Surface(Surface::Compound(Vec::new())),
        Kind::BRepSolid => Body::Solid(Vec::new()),
        Kind::GeomCollection => Body::Collection(Collection::Geometry(Vec::new())),
        Kind::MultiPoint => Body::Collection(Collection::MultiPoint(Vec::new())),
        Kind::MultiCurve => Body::Collection(Collection::MultiCurve(Vec::new())),
        Kind::MultiLineString => Body::Collection(Collection::MultiLineString(Vec::new())),
        Kind::MultiSurface => Body::Collection(Collection::MultiSurface(Vec::new())),
        Kind::MultiPolygon => Body::Collection(Collection::MultiPolygon(Vec::new())),
        Kind::Geometry | Kind::Curve | Kind::Surface | Kind::Solid => return None,
    })
}

/// The one element of a collection, a compound curve or surface, a
/// polyhedral surface or a TIN of one; none else.
fn only_element(body: &Body) -> Option<Body> {
    let one = |n: usize| n == 1;
    match body {
        Body::Collection(c) => match c {
            Collection::Geometry(parts) => parts.first().filter(|_| one(parts.len())).cloned(),
            Collection::MultiPoint(points) => points.first().filter(|_| one(points.len())).map(|p| Body::Point(p.clone())),
            Collection::MultiCurve(curves) => curves.first().filter(|_| one(curves.len())).map(|curve| Body::Curve(curve.clone())),
            Collection::MultiLineString(lines) => {
                lines.first().filter(|_| one(lines.len())).map(|l| Body::Curve(Curve::LineString(l.clone())))
            }
            Collection::MultiSurface(surfaces) => surfaces.first().filter(|_| one(surfaces.len())).map(|s| Body::Surface(s.clone())),
            Collection::MultiPolygon(polygons) => {
                polygons.first().filter(|_| one(polygons.len())).map(|p| Body::Surface(Surface::Polygon(p.clone())))
            }
        },
        Body::Curve(Curve::Compound(parts)) => parts.first().filter(|_| one(parts.len())).map(|c| Body::Curve(c.clone())),
        Body::Surface(Surface::Compound(parts)) => parts.first().filter(|_| one(parts.len())).map(|s| Body::Surface(s.clone())),
        Body::Surface(Surface::PolyhdrlSurface(patches)) => {
            patches.first().filter(|_| one(patches.len())).map(|p| Body::Surface(Surface::Polygon(p.clone())))
        }
        Body::Surface(Surface::Tin(tin)) => {
            tin.triangles.first().filter(|_| one(tin.triangles.len())).map(|t| Body::Surface(Surface::Triangle(t.clone())))
        }
        Body::Point(_) | Body::Curve(_) | Body::Surface(_) | Body::Solid(_) => None,
    }
}

/// The body of `g` cast to `target` by a row of Table 4 that makes a new
/// value of it: none if no such row takes it.
fn converted(g: &Geom, target: Kind) -> Result<Option<Body>> {
    let source = g.kind();
    let planar = matches!(source, Kind::CurvePolygon | Kind::Polygon | Kind::Triangle);
    Ok(match target {
        Kind::GeomCollection => Some(Body::Collection(Collection::Geometry(vec![g.body.clone()]))),
        Kind::MultiPoint => multi_point(&g.body),
        Kind::MultiCurve => multi_curve(&g.body),
        Kind::MultiLineString => lines(g)?,
        Kind::MultiSurface => multi_surface(&g.body),
        Kind::MultiPolygon => polygons(g, source)?,
        Kind::LineString => {
            if source.is_subtype_of(Kind::Curve) {
                Some(g.linear()?.body)
            } else {
                None
            }
        }
        Kind::CompoundCurve => match &g.body {
            Body::Curve(c) => Some(Body::Curve(Curve::Compound(vec![c.clone()]))),
            Body::Point(_) | Body::Surface(_) | Body::Solid(_) | Body::Collection(_) => None,
        },
        Kind::Polygon => polygon(g)?.map(|rings| Body::Surface(Surface::Polygon(rings))),
        Kind::CurvePolygon => {
            let linear = matches!(source, Kind::Polygon | Kind::Triangle);
            polygon(g)?
                .filter(|_| linear)
                .map(|rings| Body::Surface(Surface::CurvePolygon(rings.into_iter().map(Curve::LineString).collect())))
        }
        Kind::Triangle => triangle(g)?.map(|ring| Body::Surface(Surface::Triangle(ring))),
        Kind::PolyhdrlSurface => polyhedral(g, planar)?,
        Kind::Tin => triangle(g)?
            .filter(|_| planar)
            .map(|ring| Body::Surface(Surface::Tin(Tin { triangles: vec![ring], elements: Vec::new(), max_side_length: None }))),
        Kind::CompoundSurface => compound_surface(&g.body, source),
        Kind::Geometry
        | Kind::Point
        | Kind::Curve
        | Kind::CircularString
        | Kind::Circle
        | Kind::GeodesicString
        | Kind::EllipticalCurve
        | Kind::NurbsCurve
        | Kind::Clothoid
        | Kind::SpiralCurve
        | Kind::Surface
        | Kind::Solid
        | Kind::BRepSolid => None,
    })
}

/// A multipoint of a point, or of a collection of points alone.
fn multi_point(body: &Body) -> Option<Body> {
    match body {
        Body::Point(p) => Some(Body::Collection(Collection::MultiPoint(vec![p.clone()]))),
        Body::Collection(Collection::Geometry(parts)) => every(parts, |p| if let Body::Point(c) = p { Some(c.clone()) } else { None })
            .map(|points| Body::Collection(Collection::MultiPoint(points))),
        Body::Curve(_) | Body::Surface(_) | Body::Solid(_) | Body::Collection(_) => None,
    }
}

/// A multicurve of a curve, or of a collection of curves alone.
fn multi_curve(body: &Body) -> Option<Body> {
    match body {
        Body::Curve(c) => Some(Body::Collection(Collection::MultiCurve(vec![c.clone()]))),
        Body::Collection(Collection::Geometry(parts)) => every(parts, |p| if let Body::Curve(c) = p { Some(c.clone()) } else { None })
            .map(|curves| Body::Collection(Collection::MultiCurve(curves))),
        Body::Point(_) | Body::Surface(_) | Body::Solid(_) | Body::Collection(_) => None,
    }
}

/// A multisurface of a surface, or of a collection of surfaces alone.
fn multi_surface(body: &Body) -> Option<Body> {
    match body {
        Body::Surface(s) => Some(Body::Collection(Collection::MultiSurface(vec![s.clone()]))),
        Body::Collection(Collection::Geometry(parts)) => every(parts, |p| if let Body::Surface(s) = p { Some(s.clone()) } else { None })
            .map(|surfaces| Body::Collection(Collection::MultiSurface(surfaces))),
        Body::Point(_) | Body::Curve(_) | Body::Solid(_) | Body::Collection(_) => None,
    }
}

/// A polyhedral surface of a TIN's triangles, or of one planar polygon.
fn polyhedral(g: &Geom, planar: bool) -> Result<Option<Body>> {
    if let Body::Surface(Surface::Tin(tin)) = &g.body {
        return Ok(Some(Body::Surface(Surface::PolyhdrlSurface(tin.triangles.iter().map(|t| vec![t.clone()]).collect()))));
    }
    Ok(polygon(g)?.filter(|_| planar).map(|rings| Body::Surface(Surface::PolyhdrlSurface(vec![rings]))))
}

/// A compound surface of one curve polygon, polygon or polyhedral surface.
fn compound_surface(body: &Body, source: Kind) -> Option<Body> {
    let compounded = matches!(source, Kind::CurvePolygon | Kind::Polygon | Kind::PolyhdrlSurface);
    if let (Body::Surface(s), true) = (body, compounded) { Some(Body::Surface(Surface::Compound(vec![s.clone()]))) } else { None }
}

/// Each of `parts` as `of` takes it, if it takes them all.
fn every<T>(parts: &[Body], of: impl Fn(&Body) -> Option<T>) -> Option<Vec<T>> {
    parts.iter().map(of).collect()
}

/// [Table 4 MLS] A curve, a multicurve or a collection of curves as a
/// multilinestring, each curve linearized.
fn lines(g: &Geom) -> Result<Option<Body>> {
    let curves: Vec<Curve> = match &g.body {
        Body::Curve(c) => vec![c.clone()],
        Body::Collection(Collection::MultiCurve(curves)) => curves.clone(),
        Body::Collection(Collection::Geometry(parts)) => {
            let Some(curves) = every(parts, |p| if let Body::Curve(c) = p { Some(c.clone()) } else { None }) else { return Ok(None) };
            curves
        }
        Body::Point(_) | Body::Surface(_) | Body::Solid(_) | Body::Collection(_) => return Ok(None),
    };
    let mut out = Vec::with_capacity(curves.len());
    for c in curves {
        let Body::Curve(Curve::LineString(points)) = g.part(Body::Curve(c)).linear()?.body else { return Ok(None) };
        out.push(points);
    }
    Ok(Some(Body::Collection(Collection::MultiLineString(out))))
}

/// [Table 4 MPY] A polygon, a curve polygon, a triangle, a multisurface or
/// a collection of surfaces, each of one polygon, as a multipolygon.
fn polygons(g: &Geom, source: Kind) -> Result<Option<Body>> {
    let surfaces: Vec<Surface> = match &g.body {
        Body::Surface(s) => {
            if !matches!(source, Kind::CurvePolygon | Kind::Polygon | Kind::Triangle) {
                return Ok(None);
            }
            vec![s.clone()]
        }
        Body::Collection(Collection::MultiSurface(surfaces)) => surfaces.clone(),
        Body::Collection(Collection::Geometry(parts)) => {
            let Some(surfaces) = every(parts, |p| if let Body::Surface(s) = p { Some(s.clone()) } else { None }) else { return Ok(None) };
            surfaces
        }
        Body::Point(_) | Body::Curve(_) | Body::Solid(_) | Body::Collection(_) => return Ok(None),
    };
    let mut out = Vec::with_capacity(surfaces.len());
    for s in surfaces {
        let Some(rings) = polygon(&g.part(Body::Surface(s)))? else { return Ok(None) };
        out.push(rings);
    }
    Ok(Some(Body::Collection(Collection::MultiPolygon(out))))
}

/// The rings of a curve polygon, linearized, a polygon or a triangle.
fn polygon(g: &Geom) -> Result<Option<Rings>> {
    Ok(match &g.body {
        Body::Surface(Surface::Polygon(rings)) => Some(rings.clone()),
        Body::Surface(Surface::Triangle(ring)) => Some(vec![ring.clone()]),
        Body::Surface(Surface::CurvePolygon(_)) => match g.linear()?.body {
            Body::Surface(Surface::Polygon(rings)) => Some(rings),
            Body::Point(_) | Body::Curve(_) | Body::Surface(_) | Body::Solid(_) | Body::Collection(_) => None,
        },
        Body::Point(_) | Body::Curve(_) | Body::Surface(_) | Body::Solid(_) | Body::Collection(_) => None,
    })
}

/// [Table 3 1-LS 4-P] The ring of a polygon or curve polygon of one ring
/// of four points.
fn triangle(g: &Geom) -> Result<Option<super::Coords>> {
    let Some(rings) = polygon(g)? else { return Ok(None) };
    let four = MIN_RING.saturating_mul(g.dims.stride());
    let [ring] = rings.as_slice() else { return Ok(None) };
    Ok((ring.len() == four).then(|| ring.clone()))
}
