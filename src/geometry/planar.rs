//! \[DESIGN\] A model value's linear form of x and y (`Geom::linear`,
//! `Geom::xy`) as the planar representation D85's relationships take
//! (`shape.rs`), so a value of any type relates as its linear form does.

use super::model::{Body, Collection, Coords, Curve, Geom, Surface};
use super::shape::{self, COLLECTION, MULTILINE, MULTIPOINT, MULTIPOLYGON, Point, Shape};
use super::{Geometry, unsupported};
use crate::error::Result;

impl Geometry {
    /// The planar value of `g`, a linear value of x and y.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 of a part that is not linear, as `Geom::linear` gives
    /// none.
    pub fn from_geom(g: &Geom) -> Result<Geometry> {
        Geometry::of(body(&g.body)?, g.srid)
    }
}

fn points(c: &Coords) -> Vec<Point> {
    c.as_chunks::<2>().0.to_vec()
}

fn rings(r: &[Coords]) -> Vec<Vec<Point>> {
    r.iter().map(points).collect()
}

fn point(c: &Coords) -> Result<Shape> {
    Ok(Shape::Point(points(c).first().map(|p| shape::coordinate(*p)).transpose()?))
}

fn body(b: &Body) -> Result<Shape> {
    Ok(match b {
        Body::Point(c) => point(c)?,
        Body::Curve(Curve::LineString(c)) => Shape::Line(points(c)),
        Body::Surface(Surface::Polygon(r)) => Shape::Polygon(rings(r)),
        Body::Collection(c) => match c {
            Collection::Geometry(parts) => Shape::Multi(COLLECTION, parts.iter().map(body).collect::<Result<_>>()?),
            Collection::MultiPoint(ps) => Shape::Multi(MULTIPOINT, ps.iter().map(point).collect::<Result<_>>()?),
            Collection::MultiLineString(ls) => Shape::Multi(MULTILINE, ls.iter().map(|l| Shape::Line(points(l))).collect()),
            Collection::MultiPolygon(ps) => Shape::Multi(MULTIPOLYGON, ps.iter().map(|p| Shape::Polygon(rings(p))).collect()),
            Collection::MultiCurve(_) | Collection::MultiSurface(_) => return Err(unsupported("a value not linear")),
        },
        Body::Curve(_) | Body::Surface(_) | Body::Solid(_) => return Err(unsupported("a value not linear")),
    })
}
