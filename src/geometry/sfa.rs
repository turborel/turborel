//! [OGC 06-104r4 §7.2] The "SQL with Geometry Types" environment of
//! Simple feature access, SQL option, where it differs from SQL/MM
//! Spatial's routines, whose it otherwise takes (RULES.md D136).
//!
//! * Its type names drop the prefix ST_ [§7.2.2.2 Note]: POINT names
//!   ST_POINT, and PolyhedSurface ST_PolyhdrlSurface (`TYPE_NAMES`).
//! * Its routines are called by those names too (`SUBJECT_ROUTINES`,
//!   `CONSTRUCTORS`), as its tests call them [Annex C.3.3.3].
//! * A polyhedral surface's NumSurfaces, SURFACE and ST_Geometries, and
//!   those of ST_MultiPolygon it supports [§7.2.14], are its patches'
//!   (`patch_method`).
//! * GeometryType names a type as Simple feature access does,
//!   `MULTILINESTRING` [Annex C.3.3.3 T7], and AsText and AsBinary write
//!   the forms of OGC 06-103r4 §7 and §8, which differ from SQL/MM's for
//!   a triangle, a polyhedral surface and a TIN (`SimpleFunction`).

use super::model::{self, Kind};
use super::{GeometryFunction, kind};
use crate::error::{Code, Result, SqlError};
use crate::functions::ScalarFunction;
use crate::spatial::formats::wkt;
use crate::value::lob::Lob;
use crate::value::{DataType, Value};

/// The prefix of SQL/MM's names.
pub const PREFIX: &str = "ST_";

/// [§7.2.2.2] The SQL type names other than GEOMETRY, which names
/// ST_GEOMETRY already, and the types they name.
pub const TYPE_NAMES: [(&str, Kind); 12] = [
    ("POINT", Kind::Point),
    ("CURVE", Kind::Curve),
    ("LINESTRING", Kind::LineString),
    ("SURFACE", Kind::Surface),
    ("POLYGON", Kind::Polygon),
    ("POLYHEDSURFACE", Kind::PolyhdrlSurface),
    ("GEOMCOLLECTION", Kind::GeomCollection),
    ("MULTICURVE", Kind::MultiCurve),
    ("MULTILINESTRING", Kind::MultiLineString),
    ("MULTISURFACE", Kind::MultiSurface),
    ("MULTIPOLYGON", Kind::MultiPolygon),
    ("MULTIPOINT", Kind::MultiPoint),
];

/// [§7.2.8 to §7.2.19, Annex C.3.3.3] The routines of a geometry value
/// named without the prefix: each the method of SQL/MM of the name with
/// it, of the value as SELF.
pub const SUBJECT_ROUTINES: [&str; 51] = [
    "AREA",
    "BOUNDARY",
    "BUFFER",
    "CENTROID",
    "CONTAINS",
    "CONVEXHULL",
    "COORDDIM",
    "CROSSES",
    "DIFFERENCE",
    "DIMENSION",
    "DISJOINT",
    "DISTANCE",
    "ENDPOINT",
    "ENVELOPE",
    "EQUALS",
    "EXTERIORRING",
    "GEOMETRIES",
    "GEOMETRYN",
    "INTERIORRINGN",
    "INTERSECTION",
    "INTERSECTS",
    "IS3D",
    "ISCLOSED",
    "ISEMPTY",
    "ISMEASURED",
    "ISRING",
    "ISSIMPLE",
    "LENGTH",
    "LOCATEALONG",
    "LOCATEBETWEEN",
    "M",
    "NUMGEOMETRIES",
    "NUMINTERIORRING",
    "NUMPATCHES",
    "NUMPOINTS",
    "NUMSURFACES",
    "OVERLAPS",
    "PATCHN",
    "POINTN",
    "POINTONSURFACE",
    "RELATE",
    "SRID",
    "STARTPOINT",
    "SURFACE",
    "SYMDIFFERENCE",
    "TOUCHES",
    "UNION",
    "WITHIN",
    "X",
    "Y",
    "Z",
];

/// [§7.2.4, §7.2.5, Annex C.3.3.3] The constructors named without the
/// prefix, and SQL/MM's function each calls.
pub const CONSTRUCTORS: [(&str, &str); 23] = [
    ("WKTTOSQL", "ST_WKTTOSQL"),
    ("WKBTOSQL", "ST_WKBTOSQL"),
    ("GEOMFROMTEXT", "ST_GEOMFROMTEXT"),
    ("POINTFROMTEXT", "ST_POINTFROMTEXT"),
    ("LINEFROMTEXT", "ST_LINEFROMTEXT"),
    ("POLYFROMTEXT", "ST_POLYFROMTEXT"),
    ("MPOINTFROMTEXT", "ST_MPOINTFROMTEXT"),
    ("MLINEFROMTEXT", "ST_MLINEFROMTEXT"),
    ("MPOLYFROMTEXT", "ST_MPOLYFROMTEXT"),
    ("GEOMCOLLFROMTEXT", "ST_GEOMCOLLFROMTXT"),
    ("GEOMCOLLFROMTXT", "ST_GEOMCOLLFROMTXT"),
    ("GEOMFROMWKB", "ST_GEOMFROMWKB"),
    ("POINTFROMWKB", "ST_POINTFROMWKB"),
    ("LINEFROMWKB", "ST_LINEFROMWKB"),
    ("POLYFROMWKB", "ST_POLYFROMWKB"),
    ("MPOINTFROMWKB", "ST_MPOINTFROMWKB"),
    ("MLINEFROMWKB", "ST_MLINEFROMWKB"),
    ("MPOLYFROMWKB", "ST_MPOLYFROMWKB"),
    ("GEOMCOLLFROMWKB", "ST_GEOMCOLLFROMWKB"),
    ("BDPOLYFROMTEXT", "ST_BDPOLYFROMTEXT"),
    ("BDMPOLYFROMTEXT", "ST_BDMPOLYFROMTEXT"),
    ("BDPOLYFROMWKB", "ST_BDPOLYFROMWKB"),
    ("BDMPOLYFROMWKB", "ST_BDMPOLYFROMWKB"),
];

/// The SQL/MM method a routine named `name` without the prefix calls,
/// if it is one of `SUBJECT_ROUTINES`.
pub fn subject_routine(name: &str) -> Option<String> {
    SUBJECT_ROUTINES.contains(&name).then(|| format!("{PREFIX}{name}"))
}

/// The SQL/MM function a constructor named `name` without the prefix
/// calls, if it is one of `CONSTRUCTORS`.
pub fn constructor(name: &str) -> Option<&'static str> {
    CONSTRUCTORS.iter().find(|(n, _)| *n == name).map(|(_, target)| *target)
}

/// [§7.2.14] The method of a polyhedral surface or TIN, of declared type
/// `kind`, that the method `name` is: NumSurfaces and ST_NumGeometries
/// ST_NumPatches, SURFACE and ST_GeometryN ST_PatchN, ST_Geometries
/// ST_Patches; none of another name or type.
pub fn patch_method(kind: Kind, name: &str) -> Option<&'static str> {
    if !kind.is_subtype_of(Kind::PolyhdrlSurface) {
        return None;
    }
    match name {
        "NUMSURFACES" | "ST_NUMSURFACES" | "ST_NUMGEOMETRIES" => Some("ST_NUMPATCHES"),
        "SURFACE" | "ST_SURFACE" | "ST_GEOMETRYN" => Some("ST_PATCHN"),
        "ST_GEOMETRIES" => Some("ST_PATCHES"),
        _ => None,
    }
}

/// [§7.2.2.2, Annex C.3.3.3 T7] The name GeometryType gives a value of
/// most specific type `kind`: its SQL type name without the prefix,
/// POLYHEDSURFACE of a polyhedral surface.
pub fn type_name(kind: Kind) -> &'static str {
    if kind == Kind::PolyhdrlSurface {
        return "POLYHEDSURFACE";
    }
    kind.type_name().strip_prefix(PREFIX).unwrap_or(kind.type_name())
}

/// [§7.1.3.3 Table 4; OGC 06-103r4 §8.2.3 Table 7] The GEOMETRY_TYPE
/// code of a column of declared type `kind` before the 1000s of Z and M:
/// Table 7's of a type it lists, none of another.
pub const fn type_code(kind: Kind) -> Option<i64> {
    match kind {
        Kind::Geometry => Some(0),
        Kind::Point => Some(1),
        Kind::LineString => Some(2),
        Kind::Polygon => Some(3),
        Kind::MultiPoint => Some(4),
        Kind::MultiLineString => Some(5),
        Kind::MultiPolygon => Some(6),
        Kind::GeomCollection => Some(7),
        Kind::CircularString => Some(8),
        Kind::CompoundCurve => Some(9),
        Kind::CurvePolygon => Some(10),
        Kind::MultiCurve => Some(11),
        Kind::MultiSurface => Some(12),
        Kind::Curve => Some(13),
        Kind::Surface => Some(14),
        Kind::PolyhdrlSurface => Some(15),
        Kind::Tin => Some(16),
        Kind::Triangle => Some(17),
        Kind::Circle
        | Kind::GeodesicString
        | Kind::EllipticalCurve
        | Kind::NurbsCurve
        | Kind::Clothoid
        | Kind::SpiralCurve
        | Kind::CompoundSurface
        | Kind::Solid
        | Kind::BRepSolid => None,
    }
}

/// [§7.2.8, Annex C.3.3.3 T7 to T9] A routine of Simple feature access
/// whose result differs from SQL/MM's of the name with the prefix.
#[derive(Clone, Copy, Debug)]
pub enum SimpleFunction {
    /// GeometryType(g): the name `type_name` gives.
    GeometryType,
    /// AsText(g): the text of OGC 06-103r4 §7.
    AsText,
    /// AsBinary(g): the representation of OGC 06-103r4 §8.
    AsBinary,
}

/// The routines of `SimpleFunction`, built in.
pub(crate) fn functions() -> Vec<std::sync::Arc<dyn ScalarFunction>> {
    [SimpleFunction::GeometryType, SimpleFunction::AsText, SimpleFunction::AsBinary]
        .into_iter()
        .map(|f| -> std::sync::Arc<dyn ScalarFunction> { std::sync::Arc::new(f) })
        .collect()
}

impl SimpleFunction {
    /// SQL/MM's routine of the name with the prefix.
    const fn sql_mm(self) -> GeometryFunction {
        match self {
            Self::GeometryType => GeometryFunction::TypeName,
            Self::AsText => GeometryFunction::AsText,
            Self::AsBinary => GeometryFunction::AsBinary,
        }
    }
}

impl ScalarFunction for SimpleFunction {
    fn name(&self) -> &str {
        match self {
            Self::GeometryType => "GEOMETRYTYPE",
            Self::AsText => "ASTEXT",
            Self::AsBinary => "ASBINARY",
        }
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        self.sql_mm().return_type(args).map_err(|_| signature(self.name()))
    }

    fn argument_type(&self, position: usize, known: &[Option<DataType>]) -> Option<DataType> {
        self.sql_mm().argument_type(position, known)
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        let [g] = args else { return Err(signature(self.name())) };
        let Some(bytes) = kind::payload(g) else { return self.sql_mm().call(args) };
        let most = kind::most_specific(bytes).ok_or_else(|| super::invalid("the value's type code names no geometry type"))?;
        let differs = matches!(most, Kind::Triangle | Kind::PolyhdrlSurface | Kind::Tin);
        match (self, differs) {
            (Self::GeometryType, _) => Ok(Value::Char(type_name(most).into())),
            (Self::AsText | Self::AsBinary, false) => self.sql_mm().call(args),
            (Self::AsText, true) => Ok(Value::Lob(Lob::of_text(&wkt::write_simple_geom(&model::Geom::decode(bytes)?)?))),
            (Self::AsBinary, true) => Ok(Value::Lob(Lob::of_octets(&model::write_simple_wkb(&model::Geom::decode(bytes)?)))),
        }
    }
}

fn signature(name: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[OGC 06-104r4 §7.2.8] invalid arguments to {name}"))
}
