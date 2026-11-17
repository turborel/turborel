//! \[EXTENSION\] Scalar constructors, observers and relationships for
//! the registered GEOMETRY type, using the existing function-call syntax.
//! These free functions are not SQL/MM structured-type methods.

use super::{Geometry, UNSPECIFIED_SRID, data_type, invalid, unsupported};
use crate::error::{Code, Result, SqlError};
use crate::functions::{ScalarFunction, ScalarRef};
use crate::params::ParameterType;
use crate::value::{Binary, DOUBLE_PRECISION, DataType, Decimal, MAX_BINARY_LENGTH, MAX_CHARACTER_LENGTH, Spelled, Value};

/// SQL interchange uses the existing string limits until large objects exist.
const TEXT: DataType = DataType::varying(MAX_CHARACTER_LENGTH);
const BINARY: DataType = DataType::VarBinary { length: MAX_BINARY_LENGTH };
const INTEGER: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };
const COORDINATE: DataType = DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::DoublePrecision };
const TYPE_NAME_LENGTH: u32 = 32;
const TWO_ARGS: usize = 2;
const THREE_ARGS: usize = 3;
const FOUR_ARGS: usize = 4;
const FIVE_ARGS: usize = 5;

/// Register each function after registering [`super::GeometryType`].
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
    /// ST_INTERSECTS(left, right), integer 1 for a shared point, else 0.
    Intersects,
    /// ST_DISJOINT(left, right), integer 1 for disjoint point sets, else 0.
    Disjoint,
    /// ST_DISTANCE(left, right), planar distance, NULL for empty point sets.
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
            Self::Point => (TWO_ARGS..=THREE_ARGS).contains(&count),
            Self::MakeEnvelope => (FOUR_ARGS..=FIVE_ARGS).contains(&count),
            Self::SetSrid | Self::Intersects | Self::Disjoint | Self::Distance => count == TWO_ARGS,
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
            Self::FromText | Self::FromBinary | Self::Point | Self::MakeEnvelope | Self::SetSrid => data_type(),
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

    fn specialize(&self, args: &[Option<&Value>], budget: &std::sync::Arc<crate::memory::MemoryBudget>) -> Option<ScalarRef> {
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
        if self.paired() && !args.iter().all(|t| input.kind().comparable(t)) {
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
        // column reference proof needed for spatial candidates (D82).
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
                (Self::FromText, Value::Char(text)) => Geometry::from_wkt(text, srid).map(|g| g.to_value()),
                (Self::FromBinary, Value::Binary(bytes)) => Geometry::from_wkb(bytes.bytes(), srid).map(|g| g.to_value()),
                _ => Err(signature(self.name())),
            };
        }
        self.observe(Geometry::from_value(first)?, rest)
    }

    fn call_borrowed(&self, args: &[&Value], _temp: Option<&dyn crate::value::lob::TempLobs>) -> Result<Value> {
        if self.paired() {
            let [left, right] = args else { return Err(signature(self.name())) };
            if left.is_null() || right.is_null() {
                return Ok(Value::Null);
            }
            return self.pair(&Geometry::from_value(left)?, &Geometry::from_value(right)?);
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
    fn observe(self, g: Geometry, rest: &[Value]) -> Result<Value> {
        match self {
            Self::AsText => text(&g),
            Self::AsBinary => binary(&g),
            Self::Srid => Ok(Value::Exact(Decimal::new(i128::from(g.srid()), 0))),
            Self::IsEmpty => Ok(Value::Exact(Decimal::new(i128::from(g.is_empty()), 0))),
            Self::TypeName => Ok(Value::Char(g.shape.sql_name().to_owned())),
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
                let [right] = rest else { return Err(signature(self.name())) };
                self.pair(&g, &Geometry::from_value(right)?)
            }
            Self::FromText | Self::FromBinary | Self::Point | Self::MakeEnvelope => Err(signature(self.name())),
        }
    }
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
        let intersects = left.intersects_located(right, located)?;
        let result = if matches!(self, Self::Disjoint) { !intersects } else { intersects };
        Ok(Value::Exact(Decimal::new(i128::from(result), 0)))
    }
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

fn text(g: &Geometry) -> Result<Value> {
    let text = g.to_wkt();
    if u32::try_from(text.len()).unwrap_or(u32::MAX) > MAX_CHARACTER_LENGTH {
        return Err(unsupported("WKT exceeding the SQL character-string limit"));
    }
    Ok(Value::Char(text))
}

fn binary(g: &Geometry) -> Result<Value> {
    let bytes = g.to_wkb();
    if u32::try_from(bytes.len()).unwrap_or(u32::MAX) > MAX_BINARY_LENGTH {
        return Err(unsupported("WKB exceeding the SQL binary-string limit"));
    }
    Ok(Value::Binary(Binary::new(bytes)))
}

fn reference(args: &[Value]) -> Result<i32> {
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
