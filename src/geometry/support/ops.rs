//! The routines of the types, each a row: its name, whether it is a
//! function, a constructor or a method, its parameters, SELF first of a
//! method, its result, whether it is called on null input, whether it
//! reads the statement's catalog, and what it runs. A routine is a
//! `ScalarFunction` that declares its parameter types, so of the
//! routines of a name the binder invokes the one its arguments' types
//! choose [ISO/IEC 9075-2:2023 §9.6] (RULES.md D74).

use super::{Support, data_type};
use crate::catalog::SpatialCatalog;
use crate::error::{Code, Result, SqlError, SqlState};
use crate::functions::{CallContext, ScalarFunction};
use crate::geometry::model::{Geom, Kind};
use crate::geometry::{MAX_GEOMETRY_ARRAY_ELEMENTS, data_type_of};
use crate::spatial::srs::Fault;
use crate::value::lob::{Lob, LobKind};
use crate::value::{CharUnits, DOUBLE_PRECISION, DataType, Decimal, Spelled, Value};

/// The systems a call knows: the statement's catalog, if it has one.
pub type Systems<'a> = Option<&'a SpatialCatalog>;

/// What a routine runs: of its arguments, none null unless it is called
/// on null input, and the systems the call knows.
pub type Run = fn(&[Value], Systems<'_>) -> Result<Value>;

/// The longest text a large object argument is read to (RULES.md D85).
const MAX_LOB_INPUT: u64 = 16 * 1024 * 1024;

/// A parameter's or a result's type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ty {
    /// DOUBLE PRECISION.
    Double,
    /// INTEGER.
    Integer,
    /// SMALLINT.
    SmallInt,
    /// CHARACTER(1).
    Char1,
    /// CHARACTER VARYING of a length.
    Varchar(u32),
    /// CHARACTER LARGE OBJECT of a length.
    Clob(u32),
    /// BINARY LARGE OBJECT of a length.
    Blob(u32),
    /// One of the types.
    Of(Support),
    /// An array of one of the types.
    OfArray(Support),
    /// A geometry type.
    Geom(Kind),
    /// An array of a geometry type.
    GeomArray(Kind),
    /// DOUBLE PRECISION ARRAY of a cardinality.
    DoubleArray(u32),
}

impl Ty {
    /// Its data type.
    pub fn data_type(self) -> DataType {
        let max = MAX_GEOMETRY_ARRAY_ELEMENTS;
        match self {
            Ty::Double => DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::DoublePrecision },
            Ty::Integer => DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer },
            Ty::SmallInt => DataType::Exact { precision: 5, scale: 0, spelled: Spelled::Smallint },
            Ty::Char1 => DataType::Character { length: 1, units: CharUnits::Characters },
            Ty::Varchar(n) => DataType::varying(n),
            Ty::Clob(n) => DataType::Lob { kind: LobKind::Character, length: u64::from(n), units: CharUnits::Characters },
            Ty::Blob(n) => DataType::Lob { kind: LobKind::Binary, length: u64::from(n), units: CharUnits::Characters },
            Ty::Of(s) => data_type(s),
            Ty::OfArray(s) => DataType::array(data_type(s), max),
            Ty::Geom(k) => data_type_of(k),
            Ty::GeomArray(k) => DataType::array(data_type_of(k), max),
            Ty::DoubleArray(n) => DataType::array(Ty::Double.data_type(), n),
        }
    }
}

/// What a routine is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    /// A function, invoked by its name.
    Function,
    /// A constructor method, invoked by NEW or by its type's name.
    Constructor,
    /// A method, invoked of a value, SELF its first parameter.
    Method,
}

/// A routine.
#[derive(Clone, Copy, Debug)]
pub struct Op {
    /// Its name, upper case.
    pub name: &'static str,
    /// What it is.
    pub form: Form,
    /// Its parameters, SELF first of a method.
    pub params: &'static [Ty],
    /// Its result.
    pub result: Ty,
    /// Whether it is called on null input; else a null argument makes
    /// its result null.
    pub called: bool,
    /// Whether it reads the systems of the statement's catalog.
    pub catalog: bool,
    /// What it runs.
    pub run: Run,
}

impl Op {
    /// A method `name` of SELF and `params`.
    pub const fn method(name: &'static str, params: &'static [Ty], result: Ty, run: Run) -> Op {
        Op { name, form: Form::Method, params, result, called: false, catalog: false, run }
    }

    /// A function `name`.
    pub const fn function(name: &'static str, params: &'static [Ty], result: Ty, run: Run) -> Op {
        Op { name, form: Form::Function, params, result, called: false, catalog: false, run }
    }

    /// A constructor of `s`.
    pub const fn constructor(s: Support, params: &'static [Ty], run: Run) -> Op {
        Op { name: s.name(), form: Form::Constructor, params, result: Ty::Of(s), called: false, catalog: false, run }
    }

    /// The same routine, called on null input.
    #[must_use]
    pub const fn called(self) -> Op {
        Op { called: true, ..self }
    }

    /// The same routine, reading the statement's catalog.
    #[must_use]
    pub const fn in_catalog(self) -> Op {
        Op { catalog: true, ..self }
    }

    /// Runs it of `args`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 of other than as many arguments as it has parameters;
    /// as it raises.
    pub fn invoke(&self, args: &[Value], systems: Systems<'_>) -> Result<Value> {
        if args.len() != self.params.len() {
            return Err(SqlError::new(Code::SyntaxRule, format!("[EXTENSION] {} takes {} arguments", self.name, self.params.len())));
        }
        if !self.called && args.iter().any(Value::is_null) {
            return Ok(Value::Null);
        }
        (self.run)(args, systems)
    }
}

/// Every routine of every type.
pub fn all() -> impl Iterator<Item = &'static Op> {
    super::systems::OPS
        .iter()
        .chain(super::angles::OPS)
        .chain(super::directions::OPS)
        .chain(super::elements::OPS)
        .chain(super::vectors::OPS)
        .chain(super::placement::OPS)
        .chain(super::arrays::OPS)
}

/// A routine as a function the binder calls.
#[derive(Debug, Clone, Copy)]
pub struct Routine(pub &'static Op);

impl ScalarFunction for Routine {
    fn name(&self) -> &str {
        self.0.name
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        if args.len() != self.0.params.len() {
            return Err(SqlError::new(Code::SyntaxRule, format!("[EXTENSION] {} takes {} arguments", self.0.name, self.0.params.len())));
        }
        Ok(self.0.result.data_type())
    }

    fn argument_type(&self, position: usize, _: &[Option<DataType>]) -> Option<DataType> {
        self.0.params.get(position).map(|t| t.data_type())
    }

    fn parameter_types(&self) -> Option<Vec<DataType>> {
        Some(self.0.params.iter().map(|t| t.data_type()).collect())
    }

    fn uses_context(&self, _: usize) -> bool {
        self.0.catalog
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        self.0.invoke(args, None)
    }

    fn call_in(&self, args: &[Value], cx: &dyn CallContext) -> Result<Value> {
        self.0.invoke(args, cx.spatial())
    }
}

/// The SQL exception condition of an engine fault.
pub fn fault(f: &Fault) -> SqlError {
    super::super::routines::fault(f)
}

/// A spatial exception condition `state`, citing `clause`.
pub fn spatial(state: SqlState, clause: &str, detail: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) {clause}] {detail}")).with_state(state)
}

/// SQLCODE -301: argument `at` is not of its parameter's type.
fn argument(at: usize) -> SqlError {
    SqlError::new(Code::Data, format!("[EXTENSION] argument {} is not of its parameter's type", at.saturating_add(1)))
}

/// Argument `at`.
pub fn arg(args: &[Value], at: usize) -> Result<&Value> {
    args.get(at).ok_or_else(|| argument(at))
}

/// Argument `at`, a number.
pub fn double(args: &[Value], at: usize) -> Result<f64> {
    arg(args, at)?.as_f64().filter(|x| x.is_finite()).ok_or_else(|| argument(at))
}

/// Argument `at`, an integer.
pub fn integer(args: &[Value], at: usize) -> Result<i32> {
    let Value::Exact(d) = arg(args, at)? else { return Err(argument(at)) };
    let whole = (d.scale == 0).then_some(d.units);
    whole.and_then(|n| i32::try_from(n).ok()).ok_or_else(|| argument(at))
}

/// Argument `at`, a character string or a character large object, read
/// whole; a fixed-length string's padding is not part of it.
pub fn text(args: &[Value], at: usize) -> Result<String> {
    let v = arg(args, at)?;
    if let Value::Char(s) = v {
        return Ok(s.trim_end_matches(' ').to_owned());
    }
    let Value::Lob(lob) = v else { return Err(argument(at)) };
    if lob.kind() != LobKind::Character {
        return Err(argument(at));
    }
    let whole = lob.bounded(MAX_LOB_INPUT)?.ok_or_else(|| argument(at))?;
    String::from_utf8(whole).map_err(|_| argument(at))
}

/// Argument `at`, a binary string or a binary large object, read whole.
pub fn octets(args: &[Value], at: usize) -> Result<Vec<u8>> {
    let v = arg(args, at)?;
    if let Value::Binary(b) = v {
        return Ok(b.bytes().to_vec());
    }
    let Value::Lob(lob) = v else { return Err(argument(at)) };
    if lob.kind() != LobKind::Binary {
        return Err(argument(at));
    }
    lob.bounded(MAX_LOB_INPUT)?.ok_or_else(|| argument(at))
}

/// Argument `at`, a geometry value.
pub fn geometry(args: &[Value], at: usize) -> Result<Geom> {
    Geom::from_value(arg(args, at)?)
}

/// Argument `at`, an array: its elements.
pub fn elements(args: &[Value], at: usize) -> Result<&[Value]> {
    let Value::Array(items) = arg(args, at)? else { return Err(argument(at)) };
    Ok(items)
}

/// An INTEGER or SMALLINT result.
pub fn int(n: i64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

/// A CHARACTER LARGE OBJECT result.
pub fn clob(text: &str) -> Value {
    Value::Lob(Lob::of_text(text))
}

/// A CHARACTER VARYING result.
pub fn varchar(text: String) -> Value {
    Value::Char(text)
}
