//! The value tree the interchange formats read and write: every
//! instantiable type of ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5 to §11
//! and §17.1 over the plain coordinate types of the curve engines. It
//! mirrors the geometry model's shape part for part, so an adapter moves
//! values between the two.

use crate::spatial::curves::tin::{Constraint, ElementKind};
use crate::spatial::curves::types::{Coord, Curve, Dims};

/// A ring or point list.
pub type Ring = Vec<Coord>;
/// A polygon's rings, exterior first.
pub type Rings = Vec<Ring>;

/// One value: its coordinate dimensions, shared by every part, and its
/// body.
#[derive(Clone, Debug, PartialEq)]
pub struct Geometry {
    /// The coordinate dimensions of every point.
    pub dims: Dims,
    /// The value.
    pub body: Body,
}

/// A value of any type.
#[derive(Clone, Debug, PartialEq)]
pub enum Body {
    /// ST_Point, absent when empty.
    Point(Option<Coord>),
    /// Any curve type.
    Curve(Curve),
    /// Any surface type.
    Surface(Surface),
    /// ST_BRepSolid: its shells, each a polyhedral or compound surface.
    Solid(Vec<Surface>),
    /// Any collection type.
    Collection(Collection),
}

/// A surface of any type.
#[derive(Clone, Debug, PartialEq)]
pub enum Surface {
    /// ST_CurvePolygon: its rings, exterior first.
    CurvePolygon(Vec<Curve>),
    /// ST_Polygon.
    Polygon(Rings),
    /// ST_Triangle: a closed ring of four points, or none when empty.
    Triangle(Ring),
    /// ST_PolyhdrlSurface: its polygon patches.
    Polyhedral(Vec<Rings>),
    /// ST_TIN.
    Tin(Tin),
    /// ST_CompoundSurface: its member surfaces.
    Compound(Vec<Surface>),
}

/// An ST_TIN value (§8.6, §17.1).
#[derive(Clone, Debug, PartialEq)]
pub struct Tin {
    /// The triangle patches, each a closed ring of four points.
    pub triangles: Vec<Ring>,
    /// The elements the triangulation was built from.
    pub elements: Vec<Element>,
    /// The maximum side length, when one was given.
    pub max_side: Option<f64>,
}

/// An ST_TINElement value (§17.1).
#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    /// The element type.
    pub kind: ElementKind,
    /// The element id; 0 when the text gives none.
    pub id: i32,
    /// The element tag; empty when the text gives none.
    pub tag: String,
    /// The element's geometry.
    pub body: Body,
}

/// A collection of any type.
#[derive(Clone, Debug, PartialEq)]
pub enum Collection {
    /// ST_GeomCollection.
    Geometry(Vec<Body>),
    /// ST_MultiPoint; an empty point is absent.
    MultiPoint(Vec<Option<Coord>>),
    /// ST_MultiCurve.
    MultiCurve(Vec<Curve>),
    /// ST_MultiLineString.
    MultiLineString(Rings),
    /// ST_MultiSurface.
    MultiSurface(Vec<Surface>),
    /// ST_MultiPolygon.
    MultiPolygon(Vec<Rings>),
}

/// The instantiable types, by their well-known text keyword and base
/// well-known binary code (Table 15).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// ST_Point.
    Point,
    /// ST_LineString.
    LineString,
    /// ST_CircularString.
    CircularString,
    /// ST_Circle.
    Circle,
    /// ST_GeodesicString.
    GeodesicString,
    /// ST_EllipticalCurve.
    Elliptical,
    /// ST_NURBSCurve.
    Nurbs,
    /// ST_Clothoid.
    Clothoid,
    /// ST_SpiralCurve.
    Spiral,
    /// ST_CompoundCurve.
    CompoundCurve,
    /// ST_CurvePolygon.
    CurvePolygon,
    /// ST_Polygon.
    Polygon,
    /// ST_Triangle.
    Triangle,
    /// ST_PolyhdrlSurface.
    Polyhedral,
    /// ST_TIN.
    Tin,
    /// ST_CompoundSurface.
    CompoundSurface,
    /// ST_BRepSolid.
    BRepSolid,
    /// ST_GeomCollection.
    GeometryCollection,
    /// ST_MultiPoint.
    MultiPoint,
    /// ST_MultiCurve.
    MultiCurve,
    /// ST_MultiLineString.
    MultiLineString,
    /// ST_MultiSurface.
    MultiSurface,
    /// ST_MultiPolygon.
    MultiPolygon,
}

/// Every instantiable type.
pub const ALL: [Kind; 23] = [
    Kind::Point,
    Kind::LineString,
    Kind::CircularString,
    Kind::Circle,
    Kind::GeodesicString,
    Kind::Elliptical,
    Kind::Nurbs,
    Kind::Clothoid,
    Kind::Spiral,
    Kind::CompoundCurve,
    Kind::CurvePolygon,
    Kind::Polygon,
    Kind::Triangle,
    Kind::Polyhedral,
    Kind::Tin,
    Kind::CompoundSurface,
    Kind::BRepSolid,
    Kind::GeometryCollection,
    Kind::MultiPoint,
    Kind::MultiCurve,
    Kind::MultiLineString,
    Kind::MultiSurface,
    Kind::MultiPolygon,
];

/// The base codes of Table 15 in the order of [`ALL`]; ST_BRepSolid's
/// 1025 is its z code less the z offset.
const BASE_CODES: [u32; 23] = [1, 2, 8, 18, 19, 20, 21, 22, 23, 9, 10, 3, 17, 15, 16, 24, 25, 7, 4, 11, 5, 12, 6];

impl Kind {
    /// The keyword of §5.1.67.
    pub fn keyword(self) -> &'static str {
        match self {
            Kind::Point => "POINT",
            Kind::LineString => "LINESTRING",
            Kind::CircularString => "CIRCULARSTRING",
            Kind::Circle => "CIRCLE",
            Kind::GeodesicString => "GEODESICSTRING",
            Kind::Elliptical => "ELLIPTICALCURVE",
            Kind::Nurbs => "NURBSCURVE",
            Kind::Clothoid => "CLOTHOID",
            Kind::Spiral => "SPIRALCURVE",
            Kind::CompoundCurve => "COMPOUNDCURVE",
            Kind::CurvePolygon => "CURVEPOLYGON",
            Kind::Polygon => "POLYGON",
            Kind::Triangle => "TRIANGLE",
            Kind::Polyhedral => "POLYHEDRALSURFACE",
            Kind::Tin => "TIN",
            Kind::CompoundSurface => "COMPOUNDSURFACE",
            Kind::BRepSolid => "BREPSOLID",
            Kind::GeometryCollection => "GEOMETRYCOLLECTION",
            Kind::MultiPoint => "MULTIPOINT",
            Kind::MultiCurve => "MULTICURVE",
            Kind::MultiLineString => "MULTILINESTRING",
            Kind::MultiSurface => "MULTISURFACE",
            Kind::MultiPolygon => "MULTIPOLYGON",
        }
    }

    /// The type a keyword names, in any letter case.
    pub fn of_keyword(word: &str) -> Option<Kind> {
        ALL.into_iter().find(|k| k.keyword().eq_ignore_ascii_case(word))
    }

    /// The base code of Table 15, before the z and m offsets.
    pub fn base_code(self) -> u32 {
        ALL.iter().position(|k| *k == self).and_then(|i| BASE_CODES.get(i)).copied().unwrap_or_default()
    }
}

impl Curve {
    /// The curve's type.
    pub fn kind(&self) -> Kind {
        match self {
            Curve::LineString(_) => Kind::LineString,
            Curve::CircularString(_) => Kind::CircularString,
            Curve::Circle(_) => Kind::Circle,
            Curve::GeodesicString(_) => Kind::GeodesicString,
            Curve::Elliptical(_) => Kind::Elliptical,
            Curve::Nurbs(_) => Kind::Nurbs,
            Curve::Clothoid(_) => Kind::Clothoid,
            Curve::Spiral(_) => Kind::Spiral,
            Curve::Compound(_) => Kind::CompoundCurve,
        }
    }
}

impl Surface {
    /// The surface's type.
    pub fn kind(&self) -> Kind {
        match self {
            Surface::CurvePolygon(_) => Kind::CurvePolygon,
            Surface::Polygon(_) => Kind::Polygon,
            Surface::Triangle(_) => Kind::Triangle,
            Surface::Polyhedral(_) => Kind::Polyhedral,
            Surface::Tin(_) => Kind::Tin,
            Surface::Compound(_) => Kind::CompoundSurface,
        }
    }
}

impl Collection {
    /// The collection's type.
    pub fn kind(&self) -> Kind {
        match self {
            Collection::Geometry(_) => Kind::GeometryCollection,
            Collection::MultiPoint(_) => Kind::MultiPoint,
            Collection::MultiCurve(_) => Kind::MultiCurve,
            Collection::MultiLineString(_) => Kind::MultiLineString,
            Collection::MultiSurface(_) => Kind::MultiSurface,
            Collection::MultiPolygon(_) => Kind::MultiPolygon,
        }
    }
}

impl Body {
    /// The value's type.
    pub fn kind(&self) -> Kind {
        match self {
            Body::Point(_) => Kind::Point,
            Body::Curve(c) => c.kind(),
            Body::Surface(s) => s.kind(),
            Body::Solid(_) => Kind::BRepSolid,
            Body::Collection(c) => c.kind(),
        }
    }
}

/// The geometry an element of `kind` holds (§17.1.1 D4): a multipoint for
/// points, a polygon for areas, a line string for lines.
pub fn element_kind_of(kind: ElementKind) -> Kind {
    match kind {
        ElementKind::RandomPoints | ElementKind::GroupSpot => Kind::MultiPoint,
        ElementKind::Boundary | ElementKind::BreakVoid | ElementKind::DrapeVoid | ElementKind::Void | ElementKind::Hole => Kind::Polygon,
        ElementKind::Breakline | ElementKind::SoftBreak | ElementKind::ControlContour | ElementKind::StopLine => Kind::LineString,
    }
}

/// An element as the triangulation's constraint: its points, rings or
/// line, as point lists.
pub fn constraint(element: &Element, dims: Dims) -> Constraint {
    let parts = match &element.body {
        Body::Collection(Collection::MultiPoint(points)) => points.iter().flatten().map(|p| vec![*p]).collect(),
        Body::Surface(Surface::Polygon(rings)) => rings.clone(),
        Body::Curve(Curve::LineString(points)) => vec![points.clone()],
        Body::Point(_) | Body::Curve(_) | Body::Surface(_) | Body::Solid(_) | Body::Collection(_) => Vec::new(),
    };
    Constraint { kind: element.kind, parts, has_z: dims.has_z() }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.68 Table 15]
    /// The base codes are distinct and keywords name their types.
    #[test]
    fn codes_and_keywords() {
        let mut codes: Vec<u32> = ALL.iter().map(|k| k.base_code()).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), ALL.len());
        assert!(ALL.iter().all(|k| Kind::of_keyword(&k.keyword().to_lowercase()) == Some(*k)));
        assert_eq!(Kind::of_keyword("SQUARE"), None);
    }

    /// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §17.1.1 D4] An
    /// element's geometry becomes the triangulation's point lists.
    #[test]
    fn elements_become_constraints() {
        let points = Element {
            kind: ElementKind::RandomPoints,
            id: 1,
            tag: String::new(),
            body: Body::Collection(Collection::MultiPoint(vec![Some(Coord::xy(1.0, 2.0)), None])),
        };
        assert_eq!(constraint(&points, Dims::Xy).parts, vec![vec![Coord::xy(1.0, 2.0)]]);
        let line = Element {
            kind: ElementKind::Breakline,
            id: 2,
            tag: "a".into(),
            body: Body::Curve(Curve::LineString(vec![Coord::xy(0.0, 0.0), Coord::xy(1.0, 1.0)])),
        };
        let made = constraint(&line, Dims::Xyz);
        assert!(made.has_z && made.parts.len() == 1);
        assert_eq!(element_kind_of(ElementKind::Hole), Kind::Polygon);
        assert_eq!(constraint(&Element { body: Body::Point(None), ..line }, Dims::Xy).parts, Vec::<Vec<Coord>>::new());
    }
}
