//! \[EXTENSION\] VECTOR's scalar functions (design document §5.2, §6, §7.1),
//! called with the function-call syntax: distances (`functions/distance.rs`),
//! utilities (`functions/utility.rs`), constructors and text
//! (`functions/convert.rs`), arithmetic (`functions/arithmetic.rs`) and
//! quantization (`functions/quantize.rs`). All are deterministic and
//! never panic (§6.6). A null argument gives null, but for TO_VECTOR's
//! dimension count, format and layout, where null leaves each open.
//!
//! Metric, format, norm and layout names are character strings, as the
//! metric-first form writes them (`VECTOR_DISTANCE('cosine', a, b)`); a
//! bare key word in their place is the binder's to turn into one. The
//! operators `+ - *` of §6.5 are the binder's to turn into VECTOR_ADD,
//! VECTOR_SUBTRACT, VECTOR_MULTIPLY, VECTOR_SCALE and VECTOR_DIVIDE, and
//! the distance operators of §6.3 into the calls `OPERATORS` names.

mod arithmetic;
mod batched;
mod convert;
mod distance;
mod quantize;
mod utility;

use std::sync::Arc;

use super::metric::{COSINE, DOT, EUCLIDEAN, HAMMING, JACCARD, MANHATTAN, Metrics, built_in_metrics, metric_named};
use super::params::shape_of;
use super::{VectorRef, data_type, mismatch, vector};
use crate::error::{Code, Result, SqlError};
use crate::functions::ScalarFunction;
use crate::params::ParameterType;
use crate::value::lob::{LobKind, MAX_LOB_LENGTH};
use crate::value::{CharUnits, DOUBLE_PRECISION, DataType, MAX_CHARACTER_LENGTH, Spelled, Value};

pub use arithmetic::Arithmetic;
pub use convert::Convert;
pub use distance::Distance;
pub use quantize::Quantize;
pub use utility::Utility;

/// FLOAT64, as DOUBLE PRECISION.
pub(crate) const FLOAT64_TYPE: DataType = DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::DoublePrecision };
/// INT.
pub(crate) const INTEGER_TYPE: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };
/// VARCHAR, as long as a character string is.
pub(crate) const TEXT_TYPE: DataType = DataType::varying(MAX_CHARACTER_LENGTH);
/// CLOB.
pub(crate) const CLOB_TYPE: DataType = DataType::Lob { kind: LobKind::Character, length: MAX_LOB_LENGTH, units: CharUnits::Characters };

/// VECTOR_DISTANCE's name, which the distance operators call.
pub const VECTOR_DISTANCE: &str = "VECTOR_DISTANCE";

/// The distance operators of §6.3 and the metric each stands for, as
/// `VECTOR_DISTANCE(a, b, metric)`.
pub const OPERATORS: [(&str, &str); 6] =
    [("<->", EUCLIDEAN), ("<=>", COSINE), ("<#>", DOT), ("<+>", MANHATTAN), ("<~>", HAMMING), ("<%>", JACCARD)];

/// What an argument is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Arg {
    /// A vector, or a character string made one.
    Vector,
    /// A vector, a character string or a character large object, given
    /// as it is: TO_VECTOR's first.
    Source,
    /// A character string: a name.
    Text,
    /// A number.
    Number,
    /// An exact integer.
    Integer,
    /// An array of numbers.
    Array,
}

impl Arg {
    /// The type an argument of it is stored in, if one.
    fn declared(self) -> Option<DataType> {
        match self {
            Arg::Vector => Some(data_type()),
            Arg::Text => Some(TEXT_TYPE),
            Arg::Number => Some(FLOAT64_TYPE),
            Arg::Integer => Some(INTEGER_TYPE),
            Arg::Array | Arg::Source => None,
        }
    }

    /// Whether a value of `t` may be given as it.
    fn takes(self, t: &DataType) -> bool {
        match self {
            Arg::Vector | Arg::Source => shape_of(t).is_some() || t.is_character(),
            Arg::Text => t.is_character(),
            Arg::Number => t.is_numeric(),
            Arg::Integer => matches!(t, DataType::Exact { scale: 0, .. }),
            Arg::Array => matches!(t, DataType::Array(_)),
        }
    }
}

/// A routine of VECTOR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Routine {
    /// A distance.
    Distance(Distance),
    /// A utility.
    Utility(Utility),
    /// A constructor or text form.
    Convert(Convert),
    /// Arithmetic.
    Arithmetic(Arithmetic),
    /// Quantization.
    Quantize(Quantize),
}

impl Routine {
    fn name(self) -> &'static str {
        match self {
            Routine::Distance(r) => r.name(),
            Routine::Utility(r) => r.name(),
            Routine::Convert(r) => r.name(),
            Routine::Arithmetic(r) => r.name(),
            Routine::Quantize(r) => r.name(),
        }
    }

    /// The argument counts it takes, least and most.
    fn arity(self) -> (usize, usize) {
        match self {
            Routine::Distance(r) => r.arity(),
            Routine::Utility(r) => r.arity(),
            Routine::Convert(r) => r.arity(),
            Routine::Arithmetic(_) => (2, 2),
            Routine::Quantize(r) => r.arity(),
        }
    }

    /// What argument `at` is, the first argument a character string or not.
    fn arg(self, at: usize, text_first: bool) -> Option<Arg> {
        match self {
            Routine::Distance(r) => r.arg(at, text_first),
            Routine::Utility(r) => r.arg(at),
            Routine::Convert(r) => r.arg(at),
            Routine::Arithmetic(r) => r.arg(at),
            Routine::Quantize(r) => r.arg(at),
        }
    }

    fn output(self) -> DataType {
        match self {
            Routine::Distance(_) => FLOAT64_TYPE,
            Routine::Utility(r) => r.output(),
            Routine::Convert(r) => r.output(),
            Routine::Arithmetic(_) | Routine::Quantize(_) => data_type(),
        }
    }
}

/// A VECTOR function, with the metrics VECTOR_DISTANCE names.
#[derive(Clone)]
pub struct VectorFunction {
    routine: Routine,
    metrics: Metrics,
}

impl std::fmt::Debug for VectorFunction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.routine.name())
    }
}

/// Every routine.
fn routines() -> Vec<Routine> {
    let mut all: Vec<Routine> = Distance::ALL.into_iter().map(Routine::Distance).collect();
    all.extend(Utility::ALL.into_iter().map(Routine::Utility));
    all.extend(Convert::ALL.into_iter().map(Routine::Convert));
    all.extend(Arithmetic::ALL.into_iter().map(Routine::Arithmetic));
    all.extend(Quantize::ALL.into_iter().map(Routine::Quantize));
    all
}

/// The functions of VECTOR, VECTOR_DISTANCE naming the built-in metrics.
pub fn functions() -> Vec<Arc<dyn ScalarFunction>> {
    functions_with(&built_in_metrics())
}

/// The functions of VECTOR, VECTOR_DISTANCE naming `metrics`: the built-in
/// ones and those a database registers.
pub fn functions_with(metrics: &Metrics) -> Vec<Arc<dyn ScalarFunction>> {
    routines()
        .into_iter()
        .map(|routine| -> Arc<dyn ScalarFunction> { Arc::new(VectorFunction { routine, metrics: Arc::clone(metrics) }) })
        .collect()
}

impl VectorFunction {
    /// Its routine.
    pub fn routine(&self) -> Routine {
        self.routine
    }

    /// The metric a distance shorthand always measures by, for an index
    /// to match (R13.1); none for VECTOR_DISTANCE, whose metric is an
    /// argument, and for every other function.
    pub fn fixed_metric(&self) -> Option<&'static str> {
        match self.routine {
            Routine::Distance(d) => d.metric(),
            Routine::Utility(_) | Routine::Convert(_) | Routine::Arithmetic(_) | Routine::Quantize(_) => None,
        }
    }

    /// The metric of a call of VECTOR_DISTANCE with `constants` and the
    /// position of its first vector: of a metric named first, a constant,
    /// the vectors after it; else of a metric named third, or COSINE if
    /// none is written (R6.3). None of a metric that is no constant or
    /// that no metric registered has.
    fn named_metric(&self, constants: &[Option<&Value>]) -> Option<(String, usize)> {
        let named = |at: usize| match constants.get(at) {
            Some(Some(Value::Char(name))) => metric_named(&self.metrics, name).map(|m| m.name().to_owned()),
            Some(_) | None => None,
        };
        if matches!(constants.first(), Some(Some(Value::Char(_)))) {
            return named(0).map(|m| (m, 1));
        }
        if constants.len() < 3 {
            return Some((COSINE.to_owned(), 0));
        }
        named(2).map(|m| (m, 0))
    }
}

fn signature(name: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[EXTENSION] invalid arguments to {name}"))
}

impl ScalarFunction for VectorFunction {
    fn name(&self) -> &'static str {
        self.routine.name()
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let (least, most) = self.routine.arity();
        if args.len() < least || args.len() > most {
            return Err(signature(self.name()));
        }
        let text_first = args.first().is_some_and(DataType::is_character);
        for (at, t) in args.iter().enumerate() {
            let fits = self.routine.arg(at, text_first).is_some_and(|arg| arg.takes(t));
            if !fits {
                return Err(signature(self.name()));
            }
        }
        Ok(self.routine.output())
    }

    fn argument_type(&self, position: usize, known: &[Option<DataType>]) -> Option<DataType> {
        let text_first = known.first().and_then(Option::as_ref).is_some_and(DataType::is_character);
        // Keep a vector's own type, since storing it in plain VECTOR
        // changes nothing and would hide a column's fixed shape.
        let own = known.get(position).and_then(Option::as_ref).filter(|t| shape_of(t).is_some());
        let arg = self.routine.arg(position, text_first)?;
        match (arg, own) {
            (Arg::Vector, Some(t)) => Some(t.clone()),
            (Arg::Vector | Arg::Source | Arg::Text | Arg::Number | Arg::Integer | Arg::Array, _) => arg.declared(),
        }
    }

    /// A `?` in a distance's vector place takes the type of the vector it
    /// is measured against, if that is of a fixed shape: so a query vector
    /// given as text is read in the column's dimensions and format, as an
    /// index of the column searches by (R13.1). Elsewhere it takes the
    /// type `argument_type` declares.
    fn parameter_type(&self, position: usize, known: &[Option<DataType>]) -> Option<ParameterType> {
        let text_first = known.first().and_then(Option::as_ref).is_some_and(DataType::is_character);
        let vector_place = matches!(self.routine, Routine::Distance(_)) && self.routine.arg(position, text_first) == Some(Arg::Vector);
        if vector_place {
            let first = usize::from(text_first && matches!(self.routine, Routine::Distance(Distance::VectorDistance)));
            let other = if position == first { first.checked_add(1)? } else { first };
            let shaped = known.get(other).and_then(Option::as_ref).filter(|t| shape_of(t).is_some());
            if let Some(t) = shaped {
                return Some(ParameterType::Of(t.clone()));
            }
        }
        self.argument_type(position, known).map(ParameterType::Of)
    }

    /// A metric, format, layout, norm or property written as a key word
    /// (R6.2): VECTOR_DISTANCE's first argument only if a metric is so
    /// named, as it is otherwise a vector.
    fn word_argument(&self, position: usize, word: &str, first: bool) -> bool {
        if matches!(self.routine, Routine::Distance(Distance::VectorDistance)) && position == 0 {
            return metric_named(&self.metrics, word).is_some();
        }
        self.routine.arg(position, first) == Some(Arg::Text)
    }

    /// A distance raises only of its arguments: of vectors of other
    /// dimensions or formats, of a format its metric does not measure, of
    /// a metric no metric's name. A column of a fixed shape and a constant
    /// operand raise alike for every row, and a null vector gives null; the
    /// planner holds the call to that, and to a built-in metric, which
    /// never fails of finite vectors, before it skips a row for it
    /// (`physical/nearest/safe.rs`, D94).
    fn rechecked_errors(&self) -> bool {
        matches!(self.routine, Routine::Distance(_))
    }

    /// A shorthand's metric, but INNER_PRODUCT's, which orders greatest
    /// first; VECTOR_DISTANCE's named by a constant, in either order, or
    /// COSINE if none is written (R6.3, R13.1).
    fn vector_metric(&self, constants: &[Option<&Value>]) -> Option<(String, usize)> {
        let Routine::Distance(d) = self.routine else { return None };
        match d {
            Distance::InnerProduct => None,
            Distance::VectorDistance => self.named_metric(constants),
            Distance::L1 | Distance::L2 | Distance::Cosine | Distance::Hamming | Distance::Jaccard => d.metric().map(|m| (m.to_owned(), 0)),
        }
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        let (least, most) = self.routine.arity();
        if args.len() < least || args.len() > most {
            return Err(signature(self.name()));
        }
        let strict = !matches!(self.routine, Routine::Convert(Convert::ToVector | Convert::Vector));
        if args.first().is_some_and(Value::is_null) || (strict && args.iter().any(Value::is_null)) {
            return Ok(Value::Null);
        }
        match self.routine {
            Routine::Distance(r) => r.call(args, &self.metrics),
            Routine::Utility(r) => r.call(args),
            Routine::Convert(r) => r.call(args),
            Routine::Arithmetic(r) => r.call(args),
            Routine::Quantize(r) => r.call(args),
        }
    }

    /// VECTOR_DISTANCE gathers rows, so a metric that measures many at
    /// once, as a plug-in's does, is called once for them (`batched.rs`).
    fn batches(&self) -> bool {
        matches!(self.routine, Routine::Distance(Distance::VectorDistance))
    }

    fn call_many(&self, rows: &[Vec<Value>]) -> Result<Vec<Value>> {
        if self.batches()
            && let Some(done) = batched::call_many(rows, &self.metrics)
        {
            return done;
        }
        rows.iter().map(|r| self.call(r)).collect()
    }
}

// ---------------------------------------------------------------------------
// Arguments
// ---------------------------------------------------------------------------

/// Argument `at`, a vector.
pub(crate) fn vector_at(args: &[Value], at: usize) -> Result<VectorRef<'_>> {
    vector(args.get(at).ok_or_else(|| mismatch("a missing argument"))?)
}

/// Argument `at`, a character string, if given.
pub(crate) fn text_at(args: &[Value], at: usize) -> Result<Option<&str>> {
    match args.get(at) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Char(s)) => Ok(Some(s.trim_end_matches(' '))),
        Some(_) => Err(mismatch("expected a character string")),
    }
}

/// Argument `at`, a number, if given.
pub(crate) fn number_at(args: &[Value], at: usize) -> Result<Option<f64>> {
    match args.get(at) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_f64().map(Some).ok_or_else(|| mismatch("expected a number")),
    }
}

/// Argument `at`, an integer, if given.
pub(crate) fn integer_at(args: &[Value], at: usize) -> Result<Option<i64>> {
    let Some(x) = number_at(args, at)? else { return Ok(None) };
    super::integral(x).map(Some).ok_or_else(|| mismatch(&format!("{x} is not an integer")))
}
