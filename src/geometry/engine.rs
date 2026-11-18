//! \[DESIGN\] The bridge between the model's values (`model::Geom`) and
//! the planar engine (`spatial::planar`). A value goes to the engine as
//! its linear form of x and y (`Geom::linear`, `Geom::xy`), and a result
//! comes back as an XY value of the most specific type for what remains,
//! which `model::cast` then takes to any type the caller needs
//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.2.1.5].

use super::model::{Body, Collection, Coords, Curve, Dims, Geom, Rings, Surface};
use super::shape::{MULTILINE, MULTIPOINT, MULTIPOLYGON, Shape};
use crate::error::Result;
use crate::spatial::planar::{self, Geometry, Point};

/// The engine's value of `g`, its linear form of x and y.
///
/// # Errors
///
/// As `Geom::linear`: a curve is linearized by `CURVE_LINEARIZER`, and
/// a solid is the region of x and y its shells' patches cover.
pub fn engine_of(g: &Geom) -> Result<Geometry> {
    let planar = body(&g.linear()?.xy().body)?;
    if let (Body::Solid(_), Geometry::MultiPolygon(faces)) = (&g.body, &planar) {
        return projection(faces);
    }
    Ok(planar)
}

/// [§5.1.31 to §5.1.54] A solid with z ignored: the region its faces
/// cover in x and y, the union of those that are not edge on.
fn projection(faces: &[Vec<Vec<Point>>]) -> Result<Geometry> {
    let mut region = Geometry::Empty;
    for face in faces {
        let face = Geometry::Polygon(face.clone());
        if planar::area(&face).is_some_and(|a| a > 0.0_f64) {
            region = planar::union(&region, &face)?;
        }
    }
    Ok(region)
}

fn points(coords: &Coords) -> Vec<Point> {
    coords.as_chunks::<2>().0.to_vec()
}

fn rings(rings: &Rings) -> Vec<Vec<Point>> {
    rings.iter().map(points).collect()
}

/// The linear form of a part that is not linear, of x and y.
fn linear_of(part: &Body) -> Result<Body> {
    Ok(Geom { srid: 0, dims: Dims::Xy, body: part.clone() }.linear()?.body)
}

fn body(part: &Body) -> Result<Geometry> {
    Ok(match part {
        Body::Point(coords) => points(coords).first().map_or(Geometry::Empty, |point| Geometry::Point(*point)),
        Body::Curve(Curve::LineString(coords)) => Geometry::Line(points(coords)),
        Body::Surface(Surface::Polygon(r)) => Geometry::Polygon(rings(r)),
        Body::Collection(Collection::Geometry(parts)) => Geometry::Collection(parts.iter().map(body).collect::<Result<_>>()?),
        Body::Collection(Collection::MultiPoint(list)) => {
            Geometry::MultiPoint(list.iter().filter_map(|coords| points(coords).first().copied()).collect())
        }
        Body::Collection(Collection::MultiLineString(list)) => Geometry::MultiLine(list.iter().map(points).collect()),
        Body::Collection(Collection::MultiPolygon(list)) => Geometry::MultiPolygon(list.iter().map(rings).collect()),
        // The linear form has none of these, and takes each into one of
        // the kinds above.
        Body::Curve(_) | Body::Surface(_) | Body::Solid(_) | Body::Collection(Collection::MultiCurve(_) | Collection::MultiSurface(_)) => {
            return body(&linear_of(part)?);
        }
    })
}

/// The engine's value of a shape of the seven kinds of x and y, which
/// is the linear form of any value (`kind::decode`).
pub(super) fn of_shape(shape: &Shape) -> Geometry {
    match shape {
        Shape::Point(point) => point.map_or(Geometry::Empty, Geometry::Point),
        Shape::Line(line) => Geometry::Line(line.clone()),
        Shape::Polygon(rings) => Geometry::Polygon(rings.clone()),
        Shape::Multi(code, parts) => {
            let lines = || parts.iter().filter_map(|part| if let Shape::Line(line) = part { Some(line.clone()) } else { None });
            match *code {
                MULTIPOINT => Geometry::MultiPoint(
                    parts.iter().filter_map(|part| if let Shape::Point(point) = part { *point } else { None }).collect(),
                ),
                MULTILINE => Geometry::MultiLine(lines().collect()),
                MULTIPOLYGON => Geometry::MultiPolygon(
                    parts.iter().filter_map(|part| if let Shape::Polygon(rings) = part { Some(rings.clone()) } else { None }).collect(),
                ),
                _ => Geometry::Collection(parts.iter().map(of_shape).collect()),
            }
        }
    }
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
        // R00 of §5.1.39 Table 7, the empty set of type ST_Point.
        Geometry::Empty => Body::Point(Vec::new()),
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
