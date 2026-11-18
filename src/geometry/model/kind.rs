//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.2, §5.1.68 Table 15] The
//! geometry type hierarchy: the 27 types of §4.2, their supertypes, which
//! are instantiable, and the names and codes their values are written by.

/// [§4.2] A type of the geometry type hierarchy. ST_Geometry, ST_Curve,
/// ST_Surface and ST_Solid are not instantiable; every type is NOT FINAL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    /// [§5.1] ST_Geometry, the maximal supertype.
    Geometry,
    /// [§6.1] ST_Point.
    Point,
    /// [§7.1] ST_Curve.
    Curve,
    /// [§7.2] ST_LineString.
    LineString,
    /// [§7.3] ST_CircularString.
    CircularString,
    /// [§7.4] ST_Circle.
    Circle,
    /// [§7.5] ST_GeodesicString.
    GeodesicString,
    /// [§7.6] ST_EllipticalCurve.
    EllipticalCurve,
    /// [§7.7] ST_NURBSCurve.
    NurbsCurve,
    /// [§7.8] ST_Clothoid.
    Clothoid,
    /// [§7.9] ST_SpiralCurve.
    SpiralCurve,
    /// [§7.10] ST_CompoundCurve.
    CompoundCurve,
    /// [§8.1] ST_Surface.
    Surface,
    /// [§8.2] ST_CurvePolygon.
    CurvePolygon,
    /// [§8.3] ST_Polygon.
    Polygon,
    /// [§8.4] ST_Triangle.
    Triangle,
    /// [§8.5] ST_PolyhdrlSurface.
    PolyhdrlSurface,
    /// [§8.6] ST_TIN.
    Tin,
    /// [§8.7] ST_CompoundSurface.
    CompoundSurface,
    /// [§9.1] ST_Solid.
    Solid,
    /// [§9.2] ST_BRepSolid.
    BRepSolid,
    /// [§10.1] ST_GeomCollection.
    GeomCollection,
    /// [§10.2] ST_MultiPoint.
    MultiPoint,
    /// [§10.3] ST_MultiCurve.
    MultiCurve,
    /// [§10.4] ST_MultiLineString.
    MultiLineString,
    /// [§10.5] ST_MultiSurface.
    MultiSurface,
    /// [§10.6] ST_MultiPolygon.
    MultiPolygon,
}

/// The number of types in the hierarchy [§4.2].
pub const KINDS: usize = 27;

/// [§4.2] Every type of the hierarchy, each after its supertype.
pub const ALL: [Kind; KINDS] = [
    Kind::Geometry,
    Kind::Point,
    Kind::Curve,
    Kind::LineString,
    Kind::CircularString,
    Kind::Circle,
    Kind::GeodesicString,
    Kind::EllipticalCurve,
    Kind::NurbsCurve,
    Kind::Clothoid,
    Kind::SpiralCurve,
    Kind::CompoundCurve,
    Kind::Surface,
    Kind::CurvePolygon,
    Kind::Polygon,
    Kind::Triangle,
    Kind::PolyhdrlSurface,
    Kind::Tin,
    Kind::CompoundSurface,
    Kind::Solid,
    Kind::BRepSolid,
    Kind::GeomCollection,
    Kind::MultiPoint,
    Kind::MultiCurve,
    Kind::MultiLineString,
    Kind::MultiSurface,
    Kind::MultiPolygon,
];

/// [§4.2] The longest chain from a type to ST_Geometry, ST_Triangle's and
/// ST_TIN's and the multi types' below their collections: a walk up the
/// hierarchy takes at most this many steps (RULES.md R-CF-6).
pub const MAX_HIERARCHY_DEPTH: usize = 4;

// ---------------------------------------------------------------------------
// [§5.1.68 Table 15] The base codes of <well-known binary representation>,
// to which 1000 is added for Z, 2000 for M and 3000 for ZM.
// ---------------------------------------------------------------------------

/// `<wkbpoint>`.
pub const WKB_POINT: u32 = 1;
/// `<wkblinestring>`.
pub const WKB_LINESTRING: u32 = 2;
/// `<wkbpolygon>`.
pub const WKB_POLYGON: u32 = 3;
/// `<wkbmultipoint>`.
pub const WKB_MULTIPOINT: u32 = 4;
/// `<wkbmultilinestring>`.
pub const WKB_MULTILINESTRING: u32 = 5;
/// `<wkbmultipolygon>`.
pub const WKB_MULTIPOLYGON: u32 = 6;
/// `<wkbgeometrycollection>`.
pub const WKB_GEOMETRYCOLLECTION: u32 = 7;
/// `<wkbcircularstring>`.
pub const WKB_CIRCULARSTRING: u32 = 8;
/// `<wkbcompoundcurve>`.
pub const WKB_COMPOUNDCURVE: u32 = 9;
/// `<wkbcurvepolygon>`.
pub const WKB_CURVEPOLYGON: u32 = 10;
/// `<wkbmulticurve>`.
pub const WKB_MULTICURVE: u32 = 11;
/// `<wkbmultisurface>`.
pub const WKB_MULTISURFACE: u32 = 12;
/// `<wkbpolyhedralsurface>`.
pub const WKB_POLYHEDRALSURFACE: u32 = 15;
/// `<wkbtin>`.
pub const WKB_TIN: u32 = 16;
/// `<wkbtriangle>`.
pub const WKB_TRIANGLE: u32 = 17;
/// `<wkbcircle>`.
pub const WKB_CIRCLE: u32 = 18;
/// `<wkbgeodesicstring>`.
pub const WKB_GEODESICSTRING: u32 = 19;
/// `<wkbellipticalcurve>`.
pub const WKB_ELLIPTICALCURVE: u32 = 20;
/// `<wkbnurbscurve>`.
pub const WKB_NURBSCURVE: u32 = 21;
/// `<wkbclothoid>`.
pub const WKB_CLOTHOID: u32 = 22;
/// `<wkbspiralcurve>`.
pub const WKB_SPIRALCURVE: u32 = 23;
/// `<wkbcompoundsurface>`.
pub const WKB_COMPOUNDSURFACE: u32 = 24;
/// `<wkbbrepsolidz>` less 1000: ST_BRepSolid is written with Z alone.
pub const WKB_BREPSOLID: u32 = 25;
/// `<wkbaffineplacement>`, of ST_AffinePlacement [§17.3].
pub const WKB_AFFINEPLACEMENT: u32 = 102;
/// [Annex C] The deprecated codes of ST_CircularString, ST_CompoundCurve,
/// ST_CurvePolygon, ST_MultiCurve and ST_MultiSurface, read and never
/// written: 1000001 to 1000005 in that order.
pub const WKB_DEPRECATED_BASE: u32 = 1_000_000;

impl Kind {
    /// [§4.2] Its immediate supertype; none of ST_Geometry.
    pub const fn supertype(self) -> Option<Kind> {
        match self {
            Kind::Geometry => None,
            Kind::Point | Kind::Curve | Kind::Surface | Kind::Solid | Kind::GeomCollection => Some(Kind::Geometry),
            Kind::LineString
            | Kind::CircularString
            | Kind::Circle
            | Kind::GeodesicString
            | Kind::EllipticalCurve
            | Kind::NurbsCurve
            | Kind::Clothoid
            | Kind::SpiralCurve
            | Kind::CompoundCurve => Some(Kind::Curve),
            Kind::CurvePolygon | Kind::PolyhdrlSurface | Kind::CompoundSurface => Some(Kind::Surface),
            Kind::Polygon => Some(Kind::CurvePolygon),
            Kind::Triangle => Some(Kind::Polygon),
            Kind::Tin => Some(Kind::PolyhdrlSurface),
            Kind::BRepSolid => Some(Kind::Solid),
            Kind::MultiPoint | Kind::MultiCurve | Kind::MultiSurface => Some(Kind::GeomCollection),
            Kind::MultiLineString => Some(Kind::MultiCurve),
            Kind::MultiPolygon => Some(Kind::MultiSurface),
        }
    }

    /// [§4.2] Whether it is `other` or a subtype of it. The walk up is at
    /// most `MAX_HIERARCHY_DEPTH` steps.
    pub fn is_subtype_of(self, other: Kind) -> bool {
        let mut at = Some(self);
        for _ in 0..=MAX_HIERARCHY_DEPTH {
            let Some(k) = at else { return false };
            if k == other {
                return true;
            }
            at = k.supertype();
        }
        false
    }

    /// [§4.2] Whether values of it are made: all but ST_Geometry,
    /// ST_Curve, ST_Surface and ST_Solid.
    pub const fn is_instantiable(self) -> bool {
        !matches!(self, Kind::Geometry | Kind::Curve | Kind::Surface | Kind::Solid)
    }

    /// The type's name as the standard spells it, which ST_GeometryType
    /// gives [§5.1.4]: `ST_Point`.
    pub const fn sql_name(self) -> &'static str {
        match self {
            Kind::Geometry => "ST_Geometry",
            Kind::Point => "ST_Point",
            Kind::Curve => "ST_Curve",
            Kind::LineString => "ST_LineString",
            Kind::CircularString => "ST_CircularString",
            Kind::Circle => "ST_Circle",
            Kind::GeodesicString => "ST_GeodesicString",
            Kind::EllipticalCurve => "ST_EllipticalCurve",
            Kind::NurbsCurve => "ST_NURBSCurve",
            Kind::Clothoid => "ST_Clothoid",
            Kind::SpiralCurve => "ST_SpiralCurve",
            Kind::CompoundCurve => "ST_CompoundCurve",
            Kind::Surface => "ST_Surface",
            Kind::CurvePolygon => "ST_CurvePolygon",
            Kind::Polygon => "ST_Polygon",
            Kind::Triangle => "ST_Triangle",
            Kind::PolyhdrlSurface => "ST_PolyhdrlSurface",
            Kind::Tin => "ST_TIN",
            Kind::CompoundSurface => "ST_CompoundSurface",
            Kind::Solid => "ST_Solid",
            Kind::BRepSolid => "ST_BRepSolid",
            Kind::GeomCollection => "ST_GeomCollection",
            Kind::MultiPoint => "ST_MultiPoint",
            Kind::MultiCurve => "ST_MultiCurve",
            Kind::MultiLineString => "ST_MultiLineString",
            Kind::MultiSurface => "ST_MultiSurface",
            Kind::MultiPolygon => "ST_MultiPolygon",
        }
    }

    /// The type's name as an SQL `<identifier>` folds it, by which a
    /// `<data type>` names it: `ST_POINT`.
    pub const fn type_name(self) -> &'static str {
        match self {
            Kind::Geometry => "ST_GEOMETRY",
            Kind::Point => "ST_POINT",
            Kind::Curve => "ST_CURVE",
            Kind::LineString => "ST_LINESTRING",
            Kind::CircularString => "ST_CIRCULARSTRING",
            Kind::Circle => "ST_CIRCLE",
            Kind::GeodesicString => "ST_GEODESICSTRING",
            Kind::EllipticalCurve => "ST_ELLIPTICALCURVE",
            Kind::NurbsCurve => "ST_NURBSCURVE",
            Kind::Clothoid => "ST_CLOTHOID",
            Kind::SpiralCurve => "ST_SPIRALCURVE",
            Kind::CompoundCurve => "ST_COMPOUNDCURVE",
            Kind::Surface => "ST_SURFACE",
            Kind::CurvePolygon => "ST_CURVEPOLYGON",
            Kind::Polygon => "ST_POLYGON",
            Kind::Triangle => "ST_TRIANGLE",
            Kind::PolyhdrlSurface => "ST_POLYHDRLSURFACE",
            Kind::Tin => "ST_TIN",
            Kind::CompoundSurface => "ST_COMPOUNDSURFACE",
            Kind::Solid => "ST_SOLID",
            Kind::BRepSolid => "ST_BREPSOLID",
            Kind::GeomCollection => "ST_GEOMCOLLECTION",
            Kind::MultiPoint => "ST_MULTIPOINT",
            Kind::MultiCurve => "ST_MULTICURVE",
            Kind::MultiLineString => "ST_MULTILINESTRING",
            Kind::MultiSurface => "ST_MULTISURFACE",
            Kind::MultiPolygon => "ST_MULTIPOLYGON",
        }
    }

    /// The type a `<data type>` names, by its folded name.
    pub fn named(name: &str) -> Option<Kind> {
        ALL.into_iter().find(|k| k.type_name().eq_ignore_ascii_case(name))
    }

    /// [§5.1.67] The key word its well-known text begins with; none of a
    /// type not instantiable.
    pub const fn wkt_keyword(self) -> Option<&'static str> {
        match self {
            Kind::Geometry | Kind::Curve | Kind::Surface | Kind::Solid => None,
            Kind::Point => Some("POINT"),
            Kind::LineString => Some("LINESTRING"),
            Kind::CircularString => Some("CIRCULARSTRING"),
            Kind::Circle => Some("CIRCLE"),
            Kind::GeodesicString => Some("GEODESICSTRING"),
            Kind::EllipticalCurve => Some("ELLIPTICALCURVE"),
            Kind::NurbsCurve => Some("NURBSCURVE"),
            Kind::Clothoid => Some("CLOTHOID"),
            Kind::SpiralCurve => Some("SPIRALCURVE"),
            Kind::CompoundCurve => Some("COMPOUNDCURVE"),
            Kind::CurvePolygon => Some("CURVEPOLYGON"),
            Kind::Polygon => Some("POLYGON"),
            Kind::Triangle => Some("TRIANGLE"),
            Kind::PolyhdrlSurface => Some("POLYHEDRALSURFACE"),
            Kind::Tin => Some("TIN"),
            Kind::CompoundSurface => Some("COMPOUNDSURFACE"),
            Kind::BRepSolid => Some("BREPSOLID"),
            Kind::GeomCollection => Some("GEOMETRYCOLLECTION"),
            Kind::MultiPoint => Some("MULTIPOINT"),
            Kind::MultiCurve => Some("MULTICURVE"),
            Kind::MultiLineString => Some("MULTILINESTRING"),
            Kind::MultiSurface => Some("MULTISURFACE"),
            Kind::MultiPolygon => Some("MULTIPOLYGON"),
        }
    }

    /// [§5.1.68 Table 15] Its base code, before the 1000s of Z and M;
    /// none of a type not instantiable.
    pub const fn wkb_code(self) -> Option<u32> {
        match self {
            Kind::Geometry | Kind::Curve | Kind::Surface | Kind::Solid => None,
            Kind::Point => Some(WKB_POINT),
            Kind::LineString => Some(WKB_LINESTRING),
            Kind::CircularString => Some(WKB_CIRCULARSTRING),
            Kind::Circle => Some(WKB_CIRCLE),
            Kind::GeodesicString => Some(WKB_GEODESICSTRING),
            Kind::EllipticalCurve => Some(WKB_ELLIPTICALCURVE),
            Kind::NurbsCurve => Some(WKB_NURBSCURVE),
            Kind::Clothoid => Some(WKB_CLOTHOID),
            Kind::SpiralCurve => Some(WKB_SPIRALCURVE),
            Kind::CompoundCurve => Some(WKB_COMPOUNDCURVE),
            Kind::CurvePolygon => Some(WKB_CURVEPOLYGON),
            Kind::Polygon => Some(WKB_POLYGON),
            Kind::Triangle => Some(WKB_TRIANGLE),
            Kind::PolyhdrlSurface => Some(WKB_POLYHEDRALSURFACE),
            Kind::Tin => Some(WKB_TIN),
            Kind::CompoundSurface => Some(WKB_COMPOUNDSURFACE),
            Kind::BRepSolid => Some(WKB_BREPSOLID),
            Kind::GeomCollection => Some(WKB_GEOMETRYCOLLECTION),
            Kind::MultiPoint => Some(WKB_MULTIPOINT),
            Kind::MultiCurve => Some(WKB_MULTICURVE),
            Kind::MultiLineString => Some(WKB_MULTILINESTRING),
            Kind::MultiSurface => Some(WKB_MULTISURFACE),
            Kind::MultiPolygon => Some(WKB_MULTIPOLYGON),
        }
    }

    /// [§5.1.68 Table 15, Annex C] The instantiable type of a base code,
    /// the deprecated codes 1000001 to 1000005 among them.
    pub fn of_wkb_code(code: u32) -> Option<Kind> {
        let deprecated = [Kind::CircularString, Kind::CompoundCurve, Kind::CurvePolygon, Kind::MultiCurve, Kind::MultiSurface];
        if let Some(offset) = code.checked_sub(WKB_DEPRECATED_BASE).filter(|o| *o >= 1) {
            return usize::try_from(offset.saturating_sub(1)).ok().and_then(|i| deprecated.get(i)).copied();
        }
        ALL.into_iter().find(|k| k.wkb_code() == Some(code))
    }

    /// [§4.2] The dimension of every value of it that is not empty
    /// [§5.1.2]: 0 of a point, 1 of a curve, 2 of a surface, 3 of a solid;
    /// none of a collection, whose elements decide it.
    pub const fn dimension(self) -> Option<i16> {
        match self {
            Kind::Point => Some(0),
            Kind::Curve
            | Kind::LineString
            | Kind::CircularString
            | Kind::Circle
            | Kind::GeodesicString
            | Kind::EllipticalCurve
            | Kind::NurbsCurve
            | Kind::Clothoid
            | Kind::SpiralCurve
            | Kind::CompoundCurve => Some(1),
            Kind::Surface
            | Kind::CurvePolygon
            | Kind::Polygon
            | Kind::Triangle
            | Kind::PolyhdrlSurface
            | Kind::Tin
            | Kind::CompoundSurface => Some(2),
            Kind::Solid | Kind::BRepSolid => Some(3),
            Kind::Geometry
            | Kind::GeomCollection
            | Kind::MultiPoint
            | Kind::MultiCurve
            | Kind::MultiLineString
            | Kind::MultiSurface
            | Kind::MultiPolygon => None,
        }
    }
}
