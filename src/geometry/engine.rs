//! \[DESIGN\] The bridge between the model's values (`model::Geom`) and
//! the planar engine (`spatial::planar`). A value goes to the engine as
//! its linear form of x and y (`Geom::linear`, `Geom::xy`), and a result
//! comes back as an XY value of the most specific type for what remains,
//! which `model::cast` then takes to any type the caller needs
//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.2.1.5].

use super::model::{Body, Collection, Coords, Curve, Dims, Geom, Rings, Surface};
use super::unsupported;
use crate::error::Result;
use crate::spatial::planar::{self, Geometry, Point};

/// The engine's value of `g`, its linear form of x and y.
///
/// # Errors
///
/// SQLCODE -900 of a curve no linearizer takes yet.
pub fn engine_of(g: &Geom) -> Result<Geometry> {
    body(&g.linear()?.xy().body)
}

fn points(coords: &Coords) -> Vec<Point> {
    coords.as_chunks::<2>().0.to_vec()
}

fn rings(rings: &Rings) -> Vec<Vec<Point>> {
    rings.iter().map(points).collect()
}

fn curve(part: &Curve) -> Result<Vec<Point>> {
    match part {
        Curve::LineString(coords) => Ok(points(coords)),
        Curve::CircularString(_)
        | Curve::Circle(_)
        | Curve::GeodesicString(_)
        | Curve::Elliptical(_)
        | Curve::Nurbs(_)
        | Curve::Clothoid(_)
        | Curve::Spiral(_)
        | Curve::Compound(_) => Err(unsupported("a curve that is not linear")),
    }
}

fn surface(part: &Surface) -> Result<Vec<Vec<Point>>> {
    match part {
        Surface::Polygon(r) => Ok(rings(r)),
        Surface::CurvePolygon(_) | Surface::Triangle(_) | Surface::PolyhdrlSurface(_) | Surface::Tin(_) | Surface::Compound(_) => {
            Err(unsupported("a surface that is not a polygon"))
        }
    }
}

fn body(part: &Body) -> Result<Geometry> {
    Ok(match part {
        Body::Point(coords) => points(coords).first().map_or(Geometry::Empty, |point| Geometry::Point(*point)),
        Body::Curve(c) => Geometry::Line(curve(c)?),
        Body::Surface(s) => Geometry::Polygon(surface(s)?),
        Body::Solid(_) => return Err(unsupported("a solid in a planar relationship")),
        Body::Collection(c) => match c {
            Collection::Geometry(parts) => Geometry::Collection(parts.iter().map(body).collect::<Result<_>>()?),
            Collection::MultiPoint(list) => {
                Geometry::MultiPoint(list.iter().filter_map(|coords| points(coords).first().copied()).collect())
            }
            Collection::MultiCurve(list) => Geometry::MultiLine(list.iter().map(curve).collect::<Result<_>>()?),
            Collection::MultiLineString(list) => Geometry::MultiLine(list.iter().map(points).collect()),
            Collection::MultiSurface(list) => Geometry::MultiPolygon(list.iter().map(surface).collect::<Result<_>>()?),
            Collection::MultiPolygon(list) => Geometry::MultiPolygon(list.iter().map(rings).collect()),
        },
    })
}

/// The XY value of SRID `srid` holding the engine's result.
///
/// # Errors
///
/// SQLSTATE 2FF02 if the result is no value of its type, which the
/// engine's construction rules out.
pub fn geom_of(result: &Geometry, srid: i32) -> Result<Geom> {
    Geom::new(srid, Dims::Xy, body_of(result))
}

fn flat(points: &[Point]) -> Coords {
    points.iter().flatten().copied().collect()
}

fn flat_rings(rings: &[Vec<Point>]) -> Rings {
    rings.iter().map(|ring| flat(ring)).collect()
}

fn body_of(result: &Geometry) -> Body {
    match result {
        Geometry::Empty => Body::Collection(Collection::Geometry(Vec::new())),
        Geometry::Point(point) => Body::Point(flat(&[*point])),
        Geometry::Line(line) => Body::Curve(Curve::LineString(flat(line))),
        Geometry::Polygon(polygon) => Body::Surface(Surface::Polygon(flat_rings(polygon))),
        Geometry::MultiPoint(list) => Body::Collection(Collection::MultiPoint(list.iter().map(|point| flat(&[*point])).collect())),
        Geometry::MultiLine(list) => Body::Collection(Collection::MultiLineString(list.iter().map(|line| flat(line)).collect())),
        Geometry::MultiPolygon(list) => {
            Body::Collection(Collection::MultiPolygon(list.iter().map(|polygon| flat_rings(polygon)).collect()))
        }
        Geometry::Collection(items) => Body::Collection(Collection::Geometry(items.iter().map(body_of).collect())),
    }
}

/// Spatial equality [§5.1.43, §5.1.65 GR2] of x and y. Values of
/// different SRIDs are unequal; two empty values are equal.
///
/// # Errors
///
/// SQLCODE -900 of a curve no linearizer takes yet; -910 if noding does
/// not settle.
pub fn equal(a: &Geom, b: &Geom) -> Result<bool> {
    if a.srid != b.srid {
        return Ok(false);
    }
    Ok(planar::equals(&engine_of(a)?, &engine_of(b)?)?.unwrap_or(false))
}

#[cfg(test)]
mod tests;
