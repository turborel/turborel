//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.4, §5.1.7] The type and
//! emptiness of each part of a geometry value.

use super::{Body, Collection, Curve, Kind, Surface};

impl Body {
    /// Its type.
    pub fn kind(&self) -> Kind {
        match self {
            Body::Point(_) => Kind::Point,
            Body::Curve(c) => c.kind(),
            Body::Surface(s) => s.kind(),
            Body::Solid(_) => Kind::BRepSolid,
            Body::Collection(c) => c.kind(),
        }
    }

    /// Whether it has no point.
    pub fn is_empty(&self) -> bool {
        match self {
            Body::Point(c) => c.is_empty(),
            Body::Curve(c) => c.is_empty(),
            Body::Surface(s) => s.is_empty(),
            Body::Solid(shells) => shells.iter().all(Surface::is_empty),
            Body::Collection(c) => c.is_empty(),
        }
    }
}

impl Curve {
    /// Its type.
    pub fn kind(&self) -> Kind {
        match self {
            Curve::LineString(_) => Kind::LineString,
            Curve::CircularString(_) => Kind::CircularString,
            Curve::Circle(_) => Kind::Circle,
            Curve::GeodesicString(_) => Kind::GeodesicString,
            Curve::Elliptical(_) => Kind::EllipticalCurve,
            Curve::Nurbs(_) => Kind::NurbsCurve,
            Curve::Clothoid(_) => Kind::Clothoid,
            Curve::Spiral(_) => Kind::SpiralCurve,
            Curve::Compound(_) => Kind::CompoundCurve,
        }
    }

    /// Whether it has no point.
    pub fn is_empty(&self) -> bool {
        match self {
            Curve::LineString(c) | Curve::CircularString(c) | Curve::Circle(c) | Curve::GeodesicString(c) => c.is_empty(),
            Curve::Elliptical(e) => e.is_none(),
            Curve::Nurbs(n) => n.is_none(),
            Curve::Clothoid(c) => c.is_none(),
            Curve::Spiral(s) => s.is_none(),
            Curve::Compound(parts) => parts.iter().all(Curve::is_empty),
        }
    }
}

impl Surface {
    /// Its type.
    pub fn kind(&self) -> Kind {
        match self {
            Surface::CurvePolygon(_) => Kind::CurvePolygon,
            Surface::Polygon(_) => Kind::Polygon,
            Surface::Triangle(_) => Kind::Triangle,
            Surface::PolyhdrlSurface(_) => Kind::PolyhdrlSurface,
            Surface::Tin(_) => Kind::Tin,
            Surface::Compound(_) => Kind::CompoundSurface,
        }
    }

    /// Whether it has no point.
    pub fn is_empty(&self) -> bool {
        match self {
            Surface::CurvePolygon(rings) => rings.iter().all(Curve::is_empty),
            Surface::Polygon(rings) => rings.iter().all(Vec::is_empty),
            Surface::Triangle(c) => c.is_empty(),
            Surface::PolyhdrlSurface(patches) => patches.iter().flatten().all(Vec::is_empty),
            Surface::Tin(t) => t.triangles.iter().all(Vec::is_empty),
            Surface::Compound(parts) => parts.iter().all(Surface::is_empty),
        }
    }
}

impl Collection {
    /// Its type.
    pub fn kind(&self) -> Kind {
        match self {
            Collection::Geometry(_) => Kind::GeomCollection,
            Collection::MultiPoint(_) => Kind::MultiPoint,
            Collection::MultiCurve(_) => Kind::MultiCurve,
            Collection::MultiLineString(_) => Kind::MultiLineString,
            Collection::MultiSurface(_) => Kind::MultiSurface,
            Collection::MultiPolygon(_) => Kind::MultiPolygon,
        }
    }

    /// Whether it has no point: every element is empty.
    pub fn is_empty(&self) -> bool {
        match self {
            Collection::Geometry(parts) => parts.iter().all(Body::is_empty),
            Collection::MultiPoint(points) | Collection::MultiLineString(points) => points.iter().all(Vec::is_empty),
            Collection::MultiCurve(curves) => curves.iter().all(Curve::is_empty),
            Collection::MultiSurface(surfaces) => surfaces.iter().all(Surface::is_empty),
            Collection::MultiPolygon(polygons) => polygons.iter().flatten().all(Vec::is_empty),
        }
    }
}
