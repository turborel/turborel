//! \[EXTENSION\] Scalar constructors, observers and relationships for
//! the registered GEOMETRY type, using the existing function-call syntax.
//! These free functions are not SQL/MM structured-type methods.

use std::sync::Arc;

use super::model::{self, Kind};
use super::{Geometry, UNSPECIFIED_SRID, data_type, data_type_of, invalid, kind, unsupported};
use crate::error::{Code, Result, SqlError};
use crate::functions::{ScalarFunction, ScalarRef};
use crate::params::ParameterType;
use crate::spatial::formats::wkt;
use crate::value::lob::{Lob, LobKind};
use crate::value::{DOUBLE_PRECISION, DataType, Decimal, Spelled, Value};

/// [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.47] CHARACTER LARGE
/// OBJECT(ST_MaxGeometryAsText), what ST_AsText returns and the
/// well-known text constructors take.
pub(crate) const TEXT: DataType =
    DataType::Lob { kind: LobKind::Character, length: MAX_REPRESENTATION, units: crate::value::CharUnits::Characters };
/// [§5.1.48] BINARY LARGE OBJECT(ST_MaxGeometryAsBinary), what
/// ST_AsBinary returns and the well-known binary constructors take.
pub(crate) const BINARY: DataType =
    DataType::Lob { kind: LobKind::Binary, length: MAX_REPRESENTATION, units: crate::value::CharUnits::Characters };
/// ST_MaxGeometryAsText and ST_MaxGeometryAsBinary: the codecs' limit.
pub(crate) const MAX_REPRESENTATION: u64 = 16 * 1024 * 1024;
const INTEGER: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };
const COORDINATE: DataType = DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::DoublePrecision };
const TYPE_NAME_LENGTH: u32 = 32;
/// The most octets of a large object a constructor reads (RULES.md D85).
const MAX_LOB_INPUT: u64 = 16 * 1024 * 1024;
const TWO_ARGS: usize = 2;
const THREE_ARGS: usize = 3;
const FOUR_ARGS: usize = 4;
const FIVE_ARGS: usize = 5;

/// The functions of D85, built in (RULES.md D-SPATIAL-FAMILY).
pub const FUNCTIONS: [GeometryFunction; 19] = [
    GeometryFunction::Point,
    GeometryFunction::MakeEnvelope,
    GeometryFunction::X,
    GeometryFunction::Y,
    GeometryFunction::MinX,
    GeometryFunction::MaxX,
    GeometryFunction::MinY,
    GeometryFunction::MaxY,
    GeometryFunction::SetSrid,
    GeometryFunction::FromText,
    GeometryFunction::FromBinary,
    GeometryFunction::AsText,
    GeometryFunction::AsBinary,
    GeometryFunction::Srid,
    GeometryFunction::IsEmpty,
    GeometryFunction::TypeName,
    GeometryFunction::Intersects,
    GeometryFunction::Disjoint,
    GeometryFunction::Distance,
];

/// Functions over GEOMETRY. Constructors take text/binary and an optional
/// i32 SRID, default zero. Observers take one geometry; relationships take
/// two. All propagate NULL.
#[derive(Clone, Copy, Debug)]
pub enum GeometryFunction {
    /// ST_POINT(x, y [, srid]), with finite numeric coordinates.
    Point,
    /// ST_MAKEENVELOPE(min_x, min_y, max_x, max_y [, srid]), a
    /// rectangular polygon with finite, positive extents on both axes.
    MakeEnvelope,
    /// ST_X(point), NULL for an empty point; other kinds are refused.
    X,
    /// ST_Y(point), NULL for an empty point; other kinds are refused.
    Y,
    /// ST_MINX(geometry), NULL for an empty geometry.
    MinX,
    /// ST_MAXX(geometry), NULL for an empty geometry.
    MaxX,
    /// ST_MINY(geometry), NULL for an empty geometry.
    MinY,
    /// ST_MAXY(geometry), NULL for an empty geometry.
    MaxY,
    /// ST_SETSRID(geometry, srid), relabels without transforming.
    SetSrid,
    /// ST_GEOMFROMTEXT(text [, srid]).
    FromText,
    /// ST_GEOMFROMWKB(binary [, srid]).
    FromBinary,
    /// ST_ASTEXT(geometry), without its SRID.
    AsText,
    /// ST_ASBINARY(geometry), little-endian WKB without its SRID.
    AsBinary,
    /// ST_SRID(geometry), the opaque reference identifier.
    Srid,
    /// ST_ISEMPTY(geometry), integer 1 or 0.
    IsEmpty,
    /// ST_GEOMETRYTYPE(geometry), a SQL/MM-style name such as ST_Point.
    TypeName,
    /// ST_INTERSECTS(left, right), integer 1 for a shared point, else 0,
    /// the null value if either is empty [ISO/IEC 13249-3:2016 §5.1.48].
    Intersects,
    /// ST_DISJOINT(left, right), integer 1 for disjoint point sets, else 0,
    /// the null value if either is empty [§5.1.46].
    Disjoint,
    /// ST_DISTANCE(left, right [, unit]), planar distance, NULL for empty
    /// point sets, in the unit named if one is [ISO/IEC 13249-3:2016 §5.1.41].
    Distance,
}

impl GeometryFunction {
    fn input(self) -> DataType {
        match self {
            Self::Point | Self::MakeEnvelope => COORDINATE,
            Self::FromText => TEXT,
            Self::FromBinary => BINARY,
            Self::AsText
            | Self::AsBinary
            | Self::Srid
            | Self::IsEmpty
            | Self::TypeName
            | Self::X
            | Self::Y
            | Self::MinX
            | Self::MaxX
            | Self::MinY
            | Self::MaxY
            | Self::SetSrid
            | Self::Intersects
            | Self::Disjoint
            | Self::Distance => data_type(),
        }
    }

    fn constructs(self) -> bool {
        matches!(self, Self::FromText | Self::FromBinary)
    }

    fn arity(self, count: usize) -> bool {
        match self {
            Self::Point | Self::Distance => (TWO_ARGS..=THREE_ARGS).contains(&count),
            Self::MakeEnvelope => (FOUR_ARGS..=FIVE_ARGS).contains(&count),
            Self::SetSrid | Self::Intersects | Self::Disjoint => count == TWO_ARGS,
            Self::FromText | Self::FromBinary => (1..=TWO_ARGS).contains(&count),
            Self::AsText
            | Self::AsBinary
            | Self::Srid
            | Self::IsEmpty
            | Self::TypeName
            | Self::X
            | Self::Y
            | Self::MinX
            | Self::MaxX
            | Self::MinY
            | Self::MaxY => count == 1,
        }
    }

    fn reference_position(self) -> Option<usize> {
        match self {
            Self::Point => Some(TWO_ARGS),
            Self::MakeEnvelope => Some(FOUR_ARGS),
            Self::SetSrid | Self::FromText | Self::FromBinary => Some(1),
            Self::AsText
            | Self::AsBinary
            | Self::Srid
            | Self::IsEmpty
            | Self::TypeName
            | Self::X
            | Self::Y
            | Self::MinX
            | Self::MaxX
            | Self::MinY
            | Self::MaxY
            | Self::Intersects
            | Self::Disjoint
            | Self::Distance => None,
        }
    }

    fn output(self) -> DataType {
        match self {
            Self::Point => data_type_of(Kind::Point),
            Self::MakeEnvelope => data_type_of(Kind::Polygon),
            Self::FromText | Self::FromBinary | Self::SetSrid => data_type(),
            Self::X | Self::Y | Self::MinX | Self::MaxX | Self::MinY | Self::MaxY | Self::Distance => COORDINATE,
            Self::AsText => TEXT,
            Self::AsBinary => BINARY,
            Self::Srid | Self::IsEmpty | Self::Intersects | Self::Disjoint => INTEGER,
            Self::TypeName => DataType::varying(TYPE_NAME_LENGTH),
        }
    }

    fn coordinate_count(self) -> usize {
        if matches!(self, Self::Point) {
            TWO_ARGS
        } else if matches!(self, Self::MakeEnvelope) {
            FOUR_ARGS
        } else {
            0
        }
    }
}

impl ScalarFunction for GeometryFunction {
    /// ST_Distance's unit form reads the statement's reference systems.
    fn uses_context(&self, arity: usize) -> bool {
        matches!(self, Self::Distance) && arity == THREE_ARGS
    }

    fn call_in(&self, args: &[Value], cx: &dyn crate::functions::CallContext) -> Result<Value> {
        let ([left, right, unit], true) = (args, self.uses_context(args.len())) else { return self.call_with(args, cx.temp_lobs()) };
        if args.iter().any(Value::is_null) {
            return Ok(Value::Null);
        }
        let measured = self.pair(&Geometry::from_value(left)?, &Geometry::from_value(right)?)?;
        in_unit(measured, left, unit, cx.spatial())
    }

    fn spatial_operation(&self) -> Option<super::SpatialOperation> {
        use super::SpatialOperation;
        match self {
            Self::Intersects => Some(SpatialOperation::Intersects),
            Self::Distance => Some(SpatialOperation::Distance),
            Self::IsEmpty => Some(SpatialOperation::IsEmpty),
            Self::Point
            | Self::MakeEnvelope
            | Self::X
            | Self::Y
            | Self::MinX
            | Self::MaxX
            | Self::MinY
            | Self::MaxY
            | Self::SetSrid
            | Self::FromText
            | Self::FromBinary
            | Self::AsText
            | Self::AsBinary
            | Self::Srid
            | Self::TypeName
            | Self::Disjoint => None,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::Point => "ST_POINT",
            Self::MakeEnvelope => "ST_MAKEENVELOPE",
            Self::X => "ST_X",
            Self::Y => "ST_Y",
            Self::MinX => "ST_MINX",
            Self::MaxX => "ST_MAXX",
            Self::MinY => "ST_MINY",
            Self::MaxY => "ST_MAXY",
            Self::SetSrid => "ST_SETSRID",
            Self::FromText => "ST_GEOMFROMTEXT",
            Self::FromBinary => "ST_GEOMFROMWKB",
            Self::AsText => "ST_ASTEXT",
            Self::AsBinary => "ST_ASBINARY",
            Self::Srid => "ST_SRID",
            Self::IsEmpty => "ST_ISEMPTY",
            Self::TypeName => "ST_GEOMETRYTYPE",
            Self::Intersects => "ST_INTERSECTS",
            Self::Disjoint => "ST_DISJOINT",
            Self::Distance => "ST_DISTANCE",
        }
    }

    fn specialize(&self, args: &[Option<&Value>], budget: &Arc<crate::memory::MemoryBudget>) -> Option<ScalarRef> {
        self.paired().then(|| super::prepared::specialize(*self, args, budget)).flatten()
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        if !self.arity(args.len()) {
            return Err(signature(self.name()));
        }
        let Some(first) = args.first() else { return Err(signature(self.name())) };
        let input = self.input();
        let coordinates = self.coordinate_count();
        if coordinates != 0 {
            if !args.iter().take(coordinates).all(DataType::is_numeric) {
                return Err(signature(self.name()));
            }
        } else if !input.kind().comparable(first) {
            return Err(signature(self.name()));
        }
        if self.paired() && !args.iter().take(TWO_ARGS).all(|t| input.kind().comparable(t)) {
            return Err(signature(self.name()));
        }
        if matches!(self, Self::Distance) && args.get(TWO_ARGS).is_some_and(|unit| !unit.is_character()) {
            return Err(signature(self.name()));
        }
        if let Some(t) = self.reference_position().and_then(|at| args.get(at))
            && !matches!(t, DataType::Exact { scale: 0, .. })
        {
            return Err(signature(self.name()));
        }
        Ok(self.output())
    }

    fn argument_type(&self, position: usize, known: &[Option<DataType>]) -> Option<DataType> {
        // Type NULL operands and coerce coordinates, never round an SRID.
        if self.reference_position() == Some(position) {
            return None;
        }
        let arguments = if self.paired() { TWO_ARGS } else { self.coordinate_count().max(1) };
        if position >= arguments {
            return None;
        }
        let input = self.input();
        // Keep an already constrained geometry's descriptor: assigning
        // it to plain GEOMETRY adds a fallible wrapper and erases the
        // column reference proof needed for spatial candidates (D85).
        let constrained =
            known.get(position).and_then(Option::as_ref).filter(|t| input.kind().comparable(t) && super::fixed_srid(t).is_some());
        Some(constrained.cloned().unwrap_or(input))
    }

    fn parameter_type(&self, position: usize, _: &[Option<DataType>]) -> Option<ParameterType> {
        if self.reference_position() == Some(position) {
            return Some(ParameterType::Of(INTEGER));
        }
        self.argument_type(position, &[]).map(ParameterType::Of)
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        let [first, rest @ ..] = args else { return Err(signature(self.name())) };
        if !self.arity(args.len()) {
            return Err(signature(self.name()));
        }
        if args.iter().any(Value::is_null) {
            return Ok(Value::Null);
        }
        if matches!(self, Self::Point) {
            return point(args);
        }
        if matches!(self, Self::MakeEnvelope) {
            return rectangle(args);
        }
        if self.constructs() {
            let srid = reference(rest)?;
            return match (self, first) {
                (Self::FromText, Value::Char(text)) => wkt::read_geom(text, srid).map(|g| g.to_value()),
                (Self::FromBinary, Value::Binary(bytes)) => model::read_wkb(bytes.bytes(), srid).map(|g| g.to_value()),
                (Self::FromText | Self::FromBinary, Value::Lob(lob)) => self.of_lob(lob, srid),
                _ => Err(signature(self.name())),
            };
        }
        if let Some(v) = self.of_payload(first, rest)? {
            return Ok(v);
        }
        self.observe(Geometry::from_value(first)?, rest)
    }

    fn call_borrowed(&self, args: &[&Value], _temp: Option<&dyn crate::value::lob::TempLobs>) -> Result<Value> {
        if self.paired() {
            let [left, right, unit @ ..] = args else { return Err(signature(self.name())) };
            if !self.arity(args.len()) {
                return Err(signature(self.name()));
            }
            if args.iter().any(|value| value.is_null()) {
                return Ok(Value::Null);
            }
            let measured = self.pair(&Geometry::from_value(left)?, &Geometry::from_value(right)?)?;
            return match unit {
                [] => Ok(measured),
                [unit] => in_unit(measured, left, unit, None),
                _ => Err(signature(self.name())),
            };
        }
        match args {
            [] => self.call(&[]),
            [one] => self.call(&[(*one).clone()]),
            [left, right] => self.call(&[(*left).clone(), (*right).clone()]),
            _ => self.call(&args.iter().map(|value| (*value).clone()).collect::<Vec<_>>()),
        }
    }
}

impl GeometryFunction {
    /// [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.62, §5.1.63] The value
    /// of a CHARACTER LARGE OBJECT's well-known text or a BINARY LARGE
    /// OBJECT's well-known binary representation, read whole up to the
    /// representation's limit (RULES.md D85).
    fn of_lob(self, lob: &Lob, srid: i32) -> Result<Value> {
        let whole = || lob.bounded(MAX_LOB_INPUT)?.ok_or_else(|| unsupported("this representation size (RULES.md D85)"));
        match (self, lob.kind()) {
            (Self::FromText, LobKind::Character) => {
                let text = whole()?;
                wkt::read_geom(std::str::from_utf8(&text).map_err(|_| invalid("the text is not UTF-8"))?, srid).map(|g| g.to_value())
            }
            (Self::FromBinary, LobKind::Binary) => model::read_wkb(&whole()?, srid).map(|g| g.to_value()),
            (_, LobKind::Character | LobKind::Binary) => Err(signature(self.name())),
        }
    }

    /// What an observer reads of the payload of `g` alone, of a value of
    /// any type: its type, SRID and well-known binary representation, or
    /// it relabelled. ST_AsText of a type D85 does not write is the
    /// formats' writer's (RULES.md D-SPATIAL-CURVES).
    fn of_payload(self, g: &Value, rest: &[Value]) -> Result<Option<Value>> {
        let bytes = kind::payload(g).ok_or_else(|| invalid("expected a geometry value"))?;
        let (srid, wkb) = model::header(bytes)?;
        let most = kind::most_specific(bytes).ok_or_else(|| invalid("the value's type code names no geometry type"))?;
        let basic = wkb.get(1..5).is_some_and(|code| matches!(code, [1..=7, 0, 0, 0]));
        Ok(Some(match self {
            Self::TypeName => Value::Char(most.sql_name().to_owned()),
            Self::Srid => Value::Exact(Decimal::new(i128::from(srid), 0)),
            Self::AsBinary => Value::Lob(Lob::of_octets(wkb)),
            Self::SetSrid => {
                let mut relabelled = bytes.to_vec();
                if let Some(at) = relabelled.get_mut(1..5) {
                    at.copy_from_slice(&reference(rest)?.to_le_bytes());
                }
                kind::wrap(relabelled)
            }
            Self::IsEmpty => {
                if basic {
                    return Ok(None);
                }
                Value::Exact(Decimal::new(i128::from(model::Geom::decode(bytes)?.is_empty()), 0))
            }
            Self::AsText => {
                if basic {
                    return Ok(None);
                }
                let text = wkt::write_geom(&model::Geom::decode(bytes)?)?;
                if u64::try_from(text.len()).unwrap_or(u64::MAX) > MAX_REPRESENTATION {
                    return Err(unsupported("well-known text longer than ST_MaxGeometryAsText"));
                }
                Value::Lob(Lob::of_text(&text))
            }
            Self::X
            | Self::Y
            | Self::MinX
            | Self::MaxX
            | Self::MinY
            | Self::MaxY
            | Self::Intersects
            | Self::Disjoint
            | Self::Distance
            | Self::FromText
            | Self::FromBinary
            | Self::Point
            | Self::MakeEnvelope => return Ok(None),
        }))
    }

    fn observe(self, g: Geometry, rest: &[Value]) -> Result<Value> {
        match self {
            Self::AsText => text(&g),
            Self::AsBinary => Ok(binary(&g)),
            Self::Srid => Ok(Value::Exact(Decimal::new(i128::from(g.srid()), 0))),
            Self::IsEmpty => Ok(Value::Exact(Decimal::new(i128::from(g.is_empty()), 0))),
            Self::X => Ok(g.coordinates()?.map_or(Value::Null, |[x, _]| Value::Approx(x))),
            Self::Y => Ok(g.coordinates()?.map_or(Value::Null, |[_, y]| Value::Approx(y))),
            Self::MinX => Ok(g.bounds().map_or(Value::Null, |bounds| {
                let [x, _] = bounds.lower();
                Value::Approx(x)
            })),
            Self::MaxX => Ok(g.bounds().map_or(Value::Null, |bounds| {
                let [x, _] = bounds.upper();
                Value::Approx(x)
            })),
            Self::MinY => Ok(g.bounds().map_or(Value::Null, |bounds| {
                let [_, y] = bounds.lower();
                Value::Approx(y)
            })),
            Self::MaxY => Ok(g.bounds().map_or(Value::Null, |bounds| {
                let [_, y] = bounds.upper();
                Value::Approx(y)
            })),
            Self::SetSrid => Ok(g.with_srid(reference(rest)?).to_value()),
            Self::Intersects | Self::Disjoint | Self::Distance => {
                let [right, unit @ ..] = rest else { return Err(signature(self.name())) };
                let measured = self.pair(&g, &Geometry::from_value(right)?)?;
                match unit {
                    [] => Ok(measured),
                    [unit] => in_unit(measured, &g.to_value(), unit, None),
                    _ => Err(signature(self.name())),
                }
            }
            Self::FromText | Self::FromBinary | Self::Point | Self::MakeEnvelope | Self::TypeName => Err(signature(self.name())),
        }
    }
}

/// The functions of D85, ST_OrderingEquals and the observers of a
/// collection's elements, built in: ST_Geometries as ST_GeomCollection
/// declares it, which a function call of a collection takes.
pub(crate) fn functions() -> Vec<Arc<dyn ScalarFunction>> {
    let mut out: Vec<Arc<dyn ScalarFunction>> = FUNCTIONS.iter().map(|f| -> Arc<dyn ScalarFunction> { Arc::new(*f) }).collect();
    out.push(Arc::new(super::ordering::OrderingEquals));
    let collections = super::COLLECTION_FUNCTIONS.iter().take(3);
    out.extend(collections.map(|f| -> Arc<dyn ScalarFunction> { Arc::new(*f) }));
    out.extend(super::representations::functions());
    out.extend(super::observers::functions());
    out.extend(super::tins::functions());
    out.extend(super::SOLID_FUNCTIONS.iter().map(|f| -> Arc<dyn ScalarFunction> { Arc::new(*f) }));
    out.push(Arc::new(super::TinFromTable));
    out.push(Arc::new(super::BRepSolid));
    out.extend(super::routines::functions());
    out.extend(super::support::functions());
    out
}

impl GeometryFunction {
    fn paired(self) -> bool {
        matches!(self, Self::Intersects | Self::Disjoint | Self::Distance)
    }

    pub(super) fn pair(self, left: &Geometry, right: &Geometry) -> Result<Value> {
        self.pair_located(left, right, None)
    }

    pub(super) fn pair_located(self, left: &Geometry, right: &Geometry, located: Option<bool>) -> Result<Value> {
        if matches!(self, Self::Distance) {
            return left.distance_located(right, located).map(|distance| distance.map_or(Value::Null, Value::Approx));
        }
        let Some(intersects) = left.intersects_located(right, located)? else { return Ok(Value::Null) };
        let result = if matches!(self, Self::Disjoint) { !intersects } else { intersects };
        Ok(Value::Exact(Decimal::new(i128::from(result), 0)))
    }
}

/// ST_Distance(g, g, unit): the distance in the unit named [ISO/IEC
/// 13249-3:2016 (DIS, SC32 N2593) §5.1.41 Description 4], of the systems
/// `systems` defines beside those supplied.
fn in_unit(measured: Value, subject: &Value, unit: &Value, systems: Option<&crate::catalog::SpatialCatalog>) -> Result<Value> {
    let (Value::Approx(distance), Value::Char(name)) = (&measured, unit) else { return Ok(measured) };
    let srid = Geometry::from_value(subject)?.srid();
    super::routines::length_in(*distance, (srid, systems), name).map(Value::Approx)
}

fn point(args: &[Value]) -> Result<Value> {
    let [x, y, rest @ ..] = args else { return Err(invalid("a point needs X and Y")) };
    let x = x.as_f64().ok_or_else(|| invalid("X must be numeric"))?;
    let y = y.as_f64().ok_or_else(|| invalid("Y must be numeric"))?;
    Geometry::point(x, y, reference(rest)?).map(|g| g.to_value())
}

fn rectangle(args: &[Value]) -> Result<Value> {
    let [min_x, min_y, max_x, max_y, rest @ ..] = args else { return Err(invalid("a rectangle needs four coordinates")) };
    let coordinate = |v: &Value| v.as_f64().ok_or_else(|| invalid("a rectangle coordinate must be numeric"));
    Geometry::rectangle([coordinate(min_x)?, coordinate(min_y)?], [coordinate(max_x)?, coordinate(max_y)?], reference(rest)?)
        .map(|g| g.to_value())
}

/// Its well-known text, a CHARACTER LARGE OBJECT [§5.1.47].
fn text(g: &Geometry) -> Result<Value> {
    let text = g.to_wkt();
    if u64::try_from(text.len()).unwrap_or(u64::MAX) > MAX_REPRESENTATION {
        return Err(unsupported("well-known text longer than ST_MaxGeometryAsText"));
    }
    Ok(Value::Lob(Lob::of_text(&text)))
}

/// Its well-known binary representation, a BINARY LARGE OBJECT [§5.1.48].
fn binary(g: &Geometry) -> Value {
    Value::Lob(Lob::of_octets(&g.to_wkb()))
}

pub(super) fn reference(args: &[Value]) -> Result<i32> {
    match args {
        [] => Ok(UNSPECIFIED_SRID),
        [Value::Exact(d)] => {
            if d.scale != 0 {
                return Err(invalid("the SRID must be an integer"));
            }
            i32::try_from(d.units).map_err(|_| invalid("the SRID must fit a signed 32-bit integer"))
        }
        _ => Err(invalid("the SRID must be an integer")),
    }
}

fn signature(name: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[EXTENSION] invalid arguments to {name}"))
}
