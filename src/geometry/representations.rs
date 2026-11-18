//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.56 to §5.1.64 and each
//! type's FromText, FromWKB and FromGML functions] The routines of the
//! three representations for every instantiable type: the well-known text
//! (`crate::spatial::formats::wkt`), the well-known binary (the model's)
//! and GML (`crate::spatial::formats::gml`). A typed constructor returns
//! TREAT of ST_GeomFrom... AS its type, raising the representation's
//! exception for a value of another type. GML's srsName is resolved
//! against the reference systems the statement reads when it is bound,
//! appended as the last argument (RULES.md D-SPATIAL-CURVES).

use super::functions::{BINARY, MAX_REPRESENTATION, TEXT};
use super::model::{self, Geom, Kind};
use super::{data_type, data_type_of, kind};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::functions::{AppendedQuery, ScalarFunction};
use crate::spatial::formats::{gml, wkt};
use crate::value::lob::Lob;
use crate::value::{DataType, Spelled, Value};

/// The arguments of a constructor given an SRID.
const WITH_SRID: usize = 2;

const INTEGER: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };

/// A representation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// The well-known text representation.
    Text,
    /// The well-known binary representation.
    Binary,
    /// GML.
    Gml,
}

/// A constructor of a value from a representation: its name, its
/// representation and the type it returns.
#[derive(Clone, Copy, Debug)]
pub struct FromRepresentation {
    /// The function's name, upper case.
    pub name: &'static str,
    /// The representation it reads.
    pub format: Format,
    /// The type it returns.
    pub target: Kind,
    /// Whether it takes an SRID after the representation.
    pub with_srid: bool,
}

const fn of(name: &'static str, format: Format, target: Kind) -> FromRepresentation {
    FromRepresentation { name, format, target, with_srid: true }
}

/// [§5.1.56, §5.1.58, §5.1.60] The ToSQL methods' function forms, of one
/// argument and SRID 0 (GML: its srsName's).
const fn to_sql(name: &'static str, format: Format) -> FromRepresentation {
    FromRepresentation { name, format, target: Kind::Geometry, with_srid: false }
}

/// Every constructor of a representation, ST_GeomFromWKB apart, which
/// the functions of D85 hold.
pub const CONSTRUCTORS: [FromRepresentation; 69] = [
    to_sql("ST_WKTTOSQL", Format::Text),
    to_sql("ST_WKBTOSQL", Format::Binary),
    to_sql("ST_GMLTOSQL", Format::Gml),
    of("ST_GEOMFROMGML", Format::Gml, Kind::Geometry),
    of("ST_POINTFROMTEXT", Format::Text, Kind::Point),
    of("ST_POINTFROMWKB", Format::Binary, Kind::Point),
    of("ST_POINTFROMGML", Format::Gml, Kind::Point),
    of("ST_LINEFROMTEXT", Format::Text, Kind::LineString),
    of("ST_LINEFROMWKB", Format::Binary, Kind::LineString),
    of("ST_LINEFROMGML", Format::Gml, Kind::LineString),
    of("ST_CIRCULARFROMTXT", Format::Text, Kind::CircularString),
    of("ST_CIRCULARFROMWKB", Format::Binary, Kind::CircularString),
    of("ST_CIRCULARFROMGML", Format::Gml, Kind::CircularString),
    of("ST_CIRCLEFROMTXT", Format::Text, Kind::Circle),
    of("ST_CIRCLEFROMWKB", Format::Binary, Kind::Circle),
    of("ST_CIRCLEFROMGML", Format::Gml, Kind::Circle),
    of("ST_GEODESICFROMTXT", Format::Text, Kind::GeodesicString),
    of("ST_GEODESICFROMWKB", Format::Binary, Kind::GeodesicString),
    of("ST_GEODESICFROMGML", Format::Gml, Kind::GeodesicString),
    of("ST_ELLIPTICFROMTXT", Format::Text, Kind::EllipticalCurve),
    of("ST_ELLIPTICFROMWKB", Format::Binary, Kind::EllipticalCurve),
    of("ST_ELLIPTICFROMGML", Format::Gml, Kind::EllipticalCurve),
    of("ST_NURBSFROMTXT", Format::Text, Kind::NurbsCurve),
    of("ST_NURBSFROMWKB", Format::Binary, Kind::NurbsCurve),
    of("ST_NURBSFROMGML", Format::Gml, Kind::NurbsCurve),
    of("ST_CLOTHOIDFROMTXT", Format::Text, Kind::Clothoid),
    of("ST_CLOTHOIDFROMWKB", Format::Binary, Kind::Clothoid),
    of("ST_CLOTHOIDFROMGML", Format::Gml, Kind::Clothoid),
    of("ST_SPIRALFROMTXT", Format::Text, Kind::SpiralCurve),
    of("ST_SPIRALFROMWKB", Format::Binary, Kind::SpiralCurve),
    of("ST_SPIRALFROMGML", Format::Gml, Kind::SpiralCurve),
    of("ST_COMPOUNDFROMTXT", Format::Text, Kind::CompoundCurve),
    of("ST_COMPOUNDFROMWKB", Format::Binary, Kind::CompoundCurve),
    of("ST_COMPOUNDFROMGML", Format::Gml, Kind::CompoundCurve),
    of("ST_CPOLYFROMTEXT", Format::Text, Kind::CurvePolygon),
    of("ST_CPOLYFROMWKB", Format::Binary, Kind::CurvePolygon),
    of("ST_CPOLYFROMGML", Format::Gml, Kind::CurvePolygon),
    of("ST_POLYFROMTEXT", Format::Text, Kind::Polygon),
    of("ST_POLYFROMWKB", Format::Binary, Kind::Polygon),
    of("ST_POLYFROMGML", Format::Gml, Kind::Polygon),
    of("ST_TRIFROMTEXT", Format::Text, Kind::Triangle),
    of("ST_TRIFROMWKB", Format::Binary, Kind::Triangle),
    of("ST_TRIFROMGML", Format::Gml, Kind::Triangle),
    of("ST_PHSFROMTEXT", Format::Text, Kind::PolyhdrlSurface),
    of("ST_PHSFROMWKB", Format::Binary, Kind::PolyhdrlSurface),
    of("ST_PHSFROMGML", Format::Gml, Kind::PolyhdrlSurface),
    of("ST_TINFROMTEXT", Format::Text, Kind::Tin),
    of("ST_TINFROMWKB", Format::Binary, Kind::Tin),
    of("ST_TINFROMGML", Format::Gml, Kind::Tin),
    of("ST_COMPSURFFROMTXT", Format::Text, Kind::CompoundSurface),
    of("ST_COMPSURFFROMWKB", Format::Binary, Kind::CompoundSurface),
    of("ST_COMPSURFFROMGML", Format::Gml, Kind::CompoundSurface),
    of("ST_BREPFROMTEXT", Format::Text, Kind::BRepSolid),
    of("ST_BREPFROMWKB", Format::Binary, Kind::BRepSolid),
    of("ST_BREPFROMGML", Format::Gml, Kind::BRepSolid),
    of("ST_GEOMCOLLFROMTXT", Format::Text, Kind::GeomCollection),
    of("ST_GEOMCOLLFROMWKB", Format::Binary, Kind::GeomCollection),
    of("ST_GEOMCOLLFROMGML", Format::Gml, Kind::GeomCollection),
    of("ST_MPOINTFROMTEXT", Format::Text, Kind::MultiPoint),
    of("ST_MPOINTFROMWKB", Format::Binary, Kind::MultiPoint),
    of("ST_MPOINTFROMGML", Format::Gml, Kind::MultiPoint),
    of("ST_MCURVEFROMTEXT", Format::Text, Kind::MultiCurve),
    of("ST_MCURVEFROMWKB", Format::Binary, Kind::MultiCurve),
    of("ST_MCURVEFROMGML", Format::Gml, Kind::MultiCurve),
    of("ST_MLINEFROMTEXT", Format::Text, Kind::MultiLineString),
    of("ST_MLINEFROMWKB", Format::Binary, Kind::MultiLineString),
    of("ST_MLINEFROMGML", Format::Gml, Kind::MultiLineString),
    of("ST_MSURFACEFROMTXT", Format::Text, Kind::MultiSurface),
    of("ST_MSURFACEFROMWKB", Format::Binary, Kind::MultiSurface),
];

/// The rest: ST_MultiSurface's GML and ST_MultiPolygon's three.
pub const MORE_CONSTRUCTORS: [FromRepresentation; 4] = [
    of("ST_MSURFACEFROMGML", Format::Gml, Kind::MultiSurface),
    of("ST_MPOLYFROMTEXT", Format::Text, Kind::MultiPolygon),
    of("ST_MPOLYFROMWKB", Format::Binary, Kind::MultiPolygon),
    of("ST_MPOLYFROMGML", Format::Gml, Kind::MultiPolygon),
];

/// The SQL/MM Spatial exception of `state` for `clause`.
pub(crate) fn spatial(state: SqlState, clause: &str, msg: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) {clause}] {msg}")).with_state(state)
}

fn signature(name: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1] invalid arguments to {name}"))
}

/// The reference systems' query a GML routine is given last: each
/// system's identifier, organization, its identifier there and text.
fn systems_query() -> AppendedQuery {
    AppendedQuery {
        query:
            "SELECT ROW(SRS_ID, ORGANIZATION, ORGANIZATION_COORDSYS_ID, DEFINITION) FROM ST_INFORMTN_SCHEMA.ST_SPATIAL_REFERENCE_SYSTEMS"
                .to_owned(),
        tables: Vec::new(),
        columns: Vec::new(),
    }
}

/// The reference systems as the appended argument gives them.
pub struct Systems(Vec<(i32, Option<String>, Option<i32>, String)>);

impl Systems {
    /// The systems of the appended array `rows`.
    fn of(rows: &Value) -> Systems {
        let Value::Array(items) = rows else { return Systems(Vec::new()) };
        let int = |v: Option<&Value>| if let Some(Value::Exact(d)) = v { i32::try_from(d.units).ok() } else { None };
        let text = |v: Option<&Value>| if let Some(Value::Char(s)) = v { Some(s.clone()) } else { None };
        Systems(
            items
                .iter()
                .filter_map(|row| if let Value::Row(f) = row { Some(f) } else { None })
                .filter_map(|f| Some((int(f.first())?, text(f.get(1)), int(f.get(2)), text(f.get(3)).unwrap_or_default())))
                .collect(),
        )
    }
}

impl gml::Srs for Systems {
    fn by_text(&self, text: &str) -> Option<i32> {
        self.0.iter().find(|s| s.3 == text).map(|s| s.0)
    }

    fn by_code(&self, authority: &str, code: &str) -> Option<i32> {
        let code = code.trim().parse::<i32>().ok()?;
        self.0.iter().find(|s| s.1.as_deref().is_some_and(|o| o.eq_ignore_ascii_case(authority)) && s.2 == Some(code)).map(|s| s.0)
    }

    fn name(&self, srid: i32) -> Option<String> {
        let found = self.0.iter().find(|s| s.0 == srid)?;
        Some(match (&found.1, found.2) {
            (Some(org), Some(id)) => format!("{org}:{id}"),
            (Some(_) | None, _) => found.3.clone(),
        })
    }
}

/// The octets or characters of a representation argument, read whole up
/// to the representation's limit.
fn content(v: &Value) -> Result<Vec<u8>> {
    if let Value::Char(text) = v {
        return Ok(text.as_bytes().to_vec());
    }
    if let Value::Binary(bytes) = v {
        return Ok(bytes.bytes().to_vec());
    }
    let Value::Lob(lob) = v else { return Err(super::invalid("expected a representation")) };
    lob.bounded(MAX_REPRESENTATION)?.ok_or_else(|| super::unsupported("this representation size (RULES.md D85)"))
}

fn utf8(bytes: &[u8]) -> Result<&str> {
    std::str::from_utf8(bytes).map_err(|_| super::invalid("the text is not UTF-8"))
}

impl FromRepresentation {
    /// The value of representation `v`, its SRID `srid` if given.
    fn read(self, v: &Value, srid: Option<i32>, systems: &Systems) -> Result<Geom> {
        let bytes = content(v)?;
        let (g, state, clause) = match self.format {
            Format::Text => (wkt::read_geom(utf8(&bytes)?, srid.unwrap_or(0_i32))?, SqlState::SPATIAL_INVALID_WKT, "§5.1.62"),
            Format::Binary => (model::read_wkb(&bytes, srid.unwrap_or(0_i32))?, SqlState::SPATIAL_INVALID_WKB, "§5.1.63"),
            Format::Gml => {
                let g = gml::read_geom(utf8(&bytes)?, systems)?;
                (srid.map_or(g.clone(), |s| g.with_srid(s)), SqlState::SPATIAL_INVALID_GML, "§5.1.64")
            }
        };
        if !g.kind().is_subtype_of(self.target) {
            return Err(spatial(state, clause, &format!("{} reads a {}, no {}", self.name, g.kind().sql_name(), self.target.sql_name())));
        }
        Ok(g)
    }

    fn input(self) -> DataType {
        if self.format == Format::Binary { BINARY } else { TEXT }
    }
}

impl ScalarFunction for FromRepresentation {
    fn name(&self) -> &str {
        self.name
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let most = if self.with_srid { WITH_SRID } else { 1 };
        let appended = usize::from(self.format == Format::Gml);
        if !(1..=most.saturating_add(appended)).contains(&args.len()) {
            return Err(signature(self.name));
        }
        Ok(data_type_of(self.target))
    }

    fn argument_type(&self, position: usize, _: &[Option<DataType>]) -> Option<DataType> {
        if position == 0 {
            Some(self.input())
        } else if position == 1 && self.with_srid {
            Some(INTEGER)
        } else {
            None
        }
    }

    fn appended_query(&self, _: &[Option<&Value>]) -> Option<AppendedQuery> {
        (self.format == Format::Gml).then(systems_query)
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        let (args, systems) = if self.format == Format::Gml {
            let (last, rest) = args.split_last().ok_or_else(|| signature(self.name))?;
            (rest, Systems::of(last))
        } else {
            (args, Systems(Vec::new()))
        };
        if args.iter().any(Value::is_null) {
            return Ok(Value::Null);
        }
        let srid = match args.get(1) {
            Some(Value::Exact(d)) => Some(i32::try_from(d.units).map_err(|_| super::invalid("the SRID must fit a signed 32-bit integer"))?),
            Some(_) => return Err(signature(self.name)),
            None => None,
        };
        let first = args.first().ok_or_else(|| signature(self.name))?;
        Ok(self.read(first, srid, &systems)?.to_value())
    }

    fn deterministic(&self) -> bool {
        true
    }
}

/// [§5.1.61] ST_AsGML of every type, given the reference systems last.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AsGml;

impl ScalarFunction for AsGml {
    fn name(&self) -> &'static str {
        "ST_ASGML"
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        args.first().filter(|t| super::of_data_type(t).is_some()).ok_or_else(|| signature(self.name()))?;
        Ok(TEXT)
    }

    fn argument_type(&self, position: usize, known: &[Option<DataType>]) -> Option<DataType> {
        (position == 0).then(|| known.first().cloned().flatten().filter(|t| super::of_data_type(t).is_some()).unwrap_or_else(data_type))
    }

    fn appended_query(&self, _: &[Option<&Value>]) -> Option<AppendedQuery> {
        Some(systems_query())
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        let [first, systems] = args else { return Err(signature(self.name())) };
        if first.is_null() {
            return Ok(Value::Null);
        }
        kind::payload(first).ok_or_else(|| signature(self.name()))?;
        let text = gml::write_geom(&Geom::from_value(first)?, &Systems::of(systems))?;
        if u64::try_from(text.len()).unwrap_or(u64::MAX) > MAX_REPRESENTATION {
            return Err(super::unsupported("a representation longer than ST_MaxGeometryAsText"));
        }
        Ok(Value::Lob(Lob::of_text(&text)))
    }

    fn deterministic(&self) -> bool {
        true
    }
}

/// The routines of the representations, as functions.
pub(crate) fn functions() -> Vec<std::sync::Arc<dyn ScalarFunction>> {
    let mut out: Vec<std::sync::Arc<dyn ScalarFunction>> = Vec::new();
    for f in CONSTRUCTORS.iter().chain(MORE_CONSTRUCTORS.iter()) {
        out.push(std::sync::Arc::new(*f));
    }
    out.push(std::sync::Arc::new(AsGml));
    out
}
