//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.13, §5.1.15, §7, §8]
//! The methods of curves and surfaces over the engines of
//! `crate::spatial::curves`, each an `Observer` run as a routine of SELF
//! and its other arguments, in the coordinate plane: the curve observers
//! of §7.1 and those of each curve type (`observers/curves.rs`), the
//! surface measures, rings, patches and parts of §8
//! (`observers/surfaces.rs`) and the measure locators (RULES.md
//! D-SPATIAL-CURVES).

mod curves;
mod measures;
mod records;
mod surfaces;

use std::sync::Arc;

pub use curves::{CompoundMethod, CurveMethod, ParameterMethod, PointListMethod};
pub use measures::{Exact, exact};
pub use records::RecordMethod;
pub use surfaces::{MeasureMethod, PatchMethod, RingMethod};

use super::model::{Dims, Geom, Kind};
use super::{data_type, data_type_of, kind};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::functions::ScalarFunction;
use crate::spatial::curves::observe::Position;
use crate::spatial::formats::model::{from_geom, to_geom};
use crate::spatial::formats::tree::{Body, Geometry};
use crate::value::{DOUBLE_PRECISION, DataType, Decimal, Spelled, Value};

/// INTEGER.
pub(crate) const INTEGER: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };
/// DOUBLE PRECISION.
pub(crate) const DOUBLE: DataType = DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::DoublePrecision };
/// The length of ST_SpiralType's CHARACTER VARYING result.
const SPIRAL_TYPE_LENGTH: u32 = 128;
/// ST_MaxUnitNameLength, implementation-defined [§5.1.30 DR 1].
const UNIT_NAME_LENGTH: u32 = 128;

/// The result of a call and the warning it raises, if one.
pub type Warned = (Value, Option<SqlState>);

/// A parameter of an observer after SELF.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Parameter {
    /// An INTEGER position.
    Integer,
    /// A DOUBLE PRECISION number.
    Double,
    /// An ST_Point.
    Point,
}

impl Parameter {
    fn data_type(self) -> DataType {
        match self {
            Self::Integer => INTEGER,
            Self::Double => DOUBLE,
            Self::Point => data_type_of(Kind::Point),
        }
    }
}

/// A result type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Returns {
    /// INTEGER.
    Integer,
    /// DOUBLE PRECISION.
    Double,
    /// CHARACTER VARYING.
    Text,
    /// A geometry type.
    Of(Kind),
    /// An ARRAY of a geometry type.
    Array(Kind),
    /// A type of §16 or §17 (`geometry::support`).
    Support(super::support::Support),
    /// An ARRAY of a type of §16 or §17.
    Supports(super::support::Support),
}

impl Returns {
    fn data_type(self) -> DataType {
        match self {
            Self::Integer => INTEGER,
            Self::Double => DOUBLE,
            Self::Text => DataType::Varying { length: SPIRAL_TYPE_LENGTH, units: crate::value::CharUnits::Characters },
            Self::Of(k) => data_type_of(k),
            Self::Array(k) => DataType::array(data_type_of(k), super::MAX_GEOMETRY_ARRAY_ELEMENTS),
            Self::Support(s) => super::support::data_type(s),
            Self::Supports(s) => DataType::array(super::support::data_type(s), super::MAX_GEOMETRY_ARRAY_ELEMENTS),
        }
    }
}

/// How a unit form, given a unit name last, takes its unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    /// It has no unit form.
    None,
    /// Its DOUBLE PRECISION result is a length in the unit.
    Result,
    /// Its DOUBLE PRECISION argument after SELF is a length in the unit.
    Argument,
}

/// The reference systems a unit form reads: the statement's catalog.
type Systems<'a> = Option<&'a crate::catalog::SpatialCatalog>;

/// A method of a curve or a surface.
pub trait Observer: Copy + std::fmt::Debug + Send + Sync + 'static {
    /// Its name, upper case.
    fn name(self) -> &'static str;
    /// Its parameters after SELF.
    fn parameters(self) -> &'static [Parameter];
    /// Its result type, of SELF's declared type `of`.
    fn returns(self, of: Kind) -> Returns;
    /// Whether it raises 01F01 of a position outside the value.
    fn warns(self) -> bool;
    /// How its unit form takes its unit, if it has one.
    fn unit(self) -> Unit {
        Unit::None
    }
    /// Its result of SELF `g`, read as `value`, and the other arguments.
    ///
    /// # Errors
    ///
    /// The engines' exceptions; SQLCODE -103 of an argument of a wrong
    /// type.
    fn of(self, g: &Geom, value: &Geometry, rest: &[Value]) -> Result<Warned>;
}

/// An observer as a routine.
#[derive(Clone, Copy, Debug)]
pub struct Routine<T>(pub T);

/// The error of arguments a routine does not take.
pub(crate) fn signature(name: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7, §8] invalid arguments to {name}"))
}

impl<T: Observer> ScalarFunction for Routine<T> {
    fn name(&self) -> &str {
        self.0.name()
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let of = args.first().and_then(super::of_data_type).ok_or_else(|| signature(self.0.name()))?;
        let plain = self.0.parameters().len().saturating_add(1);
        let unit_form = self.0.unit() != Unit::None && args.len() == plain.saturating_add(1);
        if args.len() != plain && !(unit_form && args.last().is_some_and(DataType::is_character)) {
            return Err(signature(self.0.name()));
        }
        Ok(self.0.returns(of).data_type())
    }

    fn argument_type(&self, position: usize, known: &[Option<DataType>]) -> Option<DataType> {
        if position == 0 {
            return Some(known.first().cloned().flatten().filter(|t| super::of_data_type(t).is_some()).unwrap_or_else(data_type));
        }
        let at = position.saturating_sub(1);
        let unit = (self.0.unit() != Unit::None && at == self.0.parameters().len()).then(|| DataType::varying(UNIT_NAME_LENGTH));
        self.0.parameters().get(at).map(|p| p.data_type()).or(unit)
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        self.call_warned(args, None).map(|(v, _)| v)
    }

    fn warns(&self) -> bool {
        self.0.warns()
    }

    fn call_warned(&self, args: &[Value], _temp: Option<&dyn crate::value::lob::TempLobs>) -> Result<Warned> {
        self.run(args, None)
    }

    /// A unit form reads the statement's reference systems.
    fn uses_context(&self, arity: usize) -> bool {
        self.0.unit() != Unit::None && arity == self.0.parameters().len().saturating_add(2)
    }

    fn call_in(&self, args: &[Value], cx: &dyn crate::functions::CallContext) -> Result<Value> {
        self.run(args, cx.spatial()).map(|(v, _)| v)
    }

    fn deterministic(&self) -> bool {
        true
    }
}

impl<T: Observer> Routine<T> {
    /// A call of `args`, a unit form's in the reference systems `systems`.
    fn run(&self, args: &[Value], systems: Systems<'_>) -> Result<Warned> {
        let [first, rest @ ..] = args else { return Err(signature(self.0.name())) };
        if args.iter().any(Value::is_null) {
            return Ok((Value::Null, None));
        }
        kind::payload(first).ok_or_else(|| signature(self.0.name()))?;
        let g = Geom::from_value(first)?;
        let value = from_geom(&g)?;
        let (given, unit) = rest.split_at_checked(self.0.parameters().len()).unwrap_or((rest, &[]));
        let [Value::Char(unit)] = unit else { return self.0.of(&g, &value, rest) };
        let of = (g.srid, systems);
        match self.0.unit() {
            Unit::None => Err(signature(self.0.name())),
            Unit::Result => {
                let (result, warning) = self.0.of(&g, &value, given)?;
                let Value::Approx(x) = result else { return Ok((result, warning)) };
                Ok((Value::Approx(super::routines::length_in(x, of, unit)?), warning))
            }
            Unit::Argument => {
                let distance = super::routines::length_of(number(given.first(), self.0.name())?, of, unit)?;
                self.0.of(&g, &value, &[Value::Approx(distance)])
            }
        }
    }
}

/// An INTEGER value.
pub(crate) fn int(n: usize) -> Value {
    Value::Exact(Decimal::new(i128::try_from(n).unwrap_or(i128::MAX), 0))
}

/// The INTEGER 1 or 0 of `b`.
pub(crate) fn truth(b: bool) -> Value {
    int(usize::from(b))
}

/// A whole number argument.
pub(crate) fn whole(v: Option<&Value>, name: &str) -> Result<i64> {
    let Some(Value::Exact(d)) = v else { return Err(signature(name)) };
    if d.scale != 0 {
        return Err(signature(name));
    }
    i64::try_from(d.units).map_err(|_| signature(name))
}

/// A DOUBLE PRECISION argument.
pub(crate) fn number(v: Option<&Value>, name: &str) -> Result<f64> {
    let Some(Value::Approx(x)) = v else { return Err(signature(name)) };
    Ok(*x)
}

/// A DOUBLE PRECISION result, null when absent.
pub(crate) fn double(x: Option<f64>) -> Value {
    x.map_or(Value::Null, Value::Approx)
}

/// The value of `body` of `dims` in `g`'s reference system.
pub(crate) fn part(g: &Geom, dims: Dims, body: Body) -> Result<Value> {
    Ok(to_geom(&Geometry { dims, body }, g.srid)?.to_value())
}

/// The value of a lookup at a position: null and 01F01 when outside.
pub(crate) fn found<T>(at: Position<T>, value: impl FnOnce(T) -> Result<Value>) -> Result<Warned> {
    match at {
        Position::Empty => Ok((Value::Null, None)),
        Position::Invalid => Ok((Value::Null, Some(SqlState::SPATIAL_INVALID_POSITION))),
        Position::Found(item) => Ok((value(item)?, None)),
    }
}

/// An ARRAY of the values of `bodies` in `g`'s system and `dims`.
pub(crate) fn array(g: &Geom, dims: Dims, bodies: impl IntoIterator<Item = Body>) -> Result<Value> {
    let all = bodies.into_iter().map(|b| part(g, dims, b)).collect::<Result<Vec<_>>>()?;
    Ok(Value::Array(all.into()))
}

/// The routines as functions, one of each name.
pub(crate) fn functions() -> Vec<Arc<dyn ScalarFunction>> {
    let mut out: Vec<Arc<dyn ScalarFunction>> = Vec::new();
    out.extend(curves::CURVE_METHODS.iter().map(|m| -> Arc<dyn ScalarFunction> { Arc::new(Routine(*m)) }));
    out.extend(curves::POINT_LIST_METHODS.iter().map(|m| -> Arc<dyn ScalarFunction> { Arc::new(Routine(*m)) }));
    out.extend(curves::PARAMETER_METHODS.iter().map(|m| -> Arc<dyn ScalarFunction> { Arc::new(Routine(*m)) }));
    out.extend(curves::COMPOUND_METHODS.iter().map(|m| -> Arc<dyn ScalarFunction> { Arc::new(Routine(*m)) }));
    out.extend(surfaces::RING_METHODS.iter().map(|m| -> Arc<dyn ScalarFunction> { Arc::new(Routine(*m)) }));
    out.extend(surfaces::PATCH_METHODS.iter().map(|m| -> Arc<dyn ScalarFunction> { Arc::new(Routine(*m)) }));
    out.extend(records::RECORD_METHODS.iter().map(|m| -> Arc<dyn ScalarFunction> { Arc::new(Routine(*m)) }));
    out.extend(surfaces::MEASURE_METHODS.iter().map(|m| -> Arc<dyn ScalarFunction> { Arc::new(Routine(*m)) }));
    out
}
