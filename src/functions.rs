//! Functions as plugins.
//!
//! SQL-86 has exactly five set functions [X3.135-1986 §5.8], and their names are key
//! words: COUNT, SUM, AVG, MAX, MIN. They are implemented here as ordinary
//! `AggregateFunction` plugins registered under those names, so an
//! implementation can be swapped (say, a more accurate SUM) without touching
//! the parser, planner or executor.
//!
//! \[EXTENSION\] SQL-86 has no syntax for calling any other function. As an opt-in,
//! non-standard extension ([X3.135-1986 §3.4] allows "user options to process nonconforming
//! SQL language"), a session can accept `<identifier>`(`<args>`) and resolve it
//! to a registered `ScalarFunction` or `AggregateFunction`.
//!
//! The engine, not the plugin, applies [X3.135-1986 §5.8 GR1, GR2]: nulls are removed from
//! the argument, and duplicates too for DISTINCT, before `update` is called.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use crate::error::{Code, Result, SqlError, fail};
use crate::lexer::kw;
use crate::params::ParameterType;
use crate::value::{ArithOp, DOUBLE_PRECISION, DataType, Decimal, MAX_EXACT_PRECISION, Value, arith, compare};

mod strings;

pub use strings::CONCATENATION;

/// \[EXTENSION\] A function callable with the opt-in call syntax.
///
/// Contract (RULES.md R-SAFE-3):
/// * `name` is an upper-case `<identifier>`, not an aggregate's.
/// * `call` returns null or a value of the type `return_type` gave: a
///   CHARACTER no longer than its length, a number within its precision and
///   scale. A longer string is a data exception when stored.
/// * `deterministic` is true only if equal arguments always give equal results.
/// * Methods may be called from several threads at once (`Send + Sync`): an
///   implementation guards its own state.
/// * No method may call back into the `Database` or its sessions (RULES.md
///   R-ERR-4); the `Database` docs say what happens if one does.
pub trait ScalarFunction: Send + Sync {
    /// The name a call uses.
    fn name(&self) -> &str;
    /// Check the argument types and give the result type (called when binding).
    ///
    /// # Errors
    ///
    /// The arguments are not ones the function accepts: the statement is refused.
    fn return_type(&self, args: &[DataType]) -> Result<DataType>;
    /// Called once per row. Arguments may be null.
    ///
    /// # Errors
    ///
    /// The value cannot be computed: the statement fails with this error.
    fn call(&self, args: &[Value]) -> Result<Value>;
    /// Same arguments always give the same result? If so, the optimizer may
    /// evaluate calls with constant arguments once, at planning time.
    fn deterministic(&self) -> bool {
        true
    }
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §17.6 GR5] The type a `<dynamic
    /// parameter specification>` takes as argument `position`, from 0, the
    /// other arguments' types given in `args`, none for each that is itself
    /// one; none if it takes none there, where it is refused \[GR4\]. By
    /// default none: a function says where its arguments may be one.
    fn parameter_type(&self, _position: usize, _args: &[Option<DataType>]) -> Option<ParameterType> {
        None
    }
}

/// \[EXTENSION\] A set function: one of COUNT, SUM, AVG, MAX, MIN
/// [X3.135-1986 §5.8], replaced, or another callable with the call syntax.
///
/// Contract (RULES.md R-SAFE-3):
/// * `name` is an upper-case `<identifier>`, not a scalar function's.
/// * `accumulator` returns a fresh accumulator for one group.
/// * The caller removes nulls, and for DISTINCT duplicates, before `update`
///   [X3.135-1986 §5.8 GR1, GR2]; with no values, `finish` gives the result for
///   an empty set [X3.135-1986 §5.8 GR4].
/// * Methods may be called from several threads at once (`Send + Sync`): an
///   implementation guards its own state.
/// * No method may call back into the `Database` or its sessions (RULES.md
///   R-ERR-4); the `Database` docs say what happens if one does.
pub trait AggregateFunction: Send + Sync {
    /// The name a call uses.
    fn name(&self) -> &str;
    /// Check the argument type and give the result type. `None` is COUNT(*).
    ///
    /// # Errors
    ///
    /// The argument is not one the function accepts: the statement is refused.
    fn result_type(&self, arg: Option<DataType>) -> Result<DataType>;
    /// A fresh accumulator for one group, whose result has type `result`.
    fn accumulator(&self, result: DataType) -> Box<dyn Accumulator>;
}

/// The running state of a set function over one group.
pub trait Accumulator {
    /// Called once per value of the argument set S (never null).
    /// For COUNT(*), called once per row with a null placeholder.
    ///
    /// # Errors
    ///
    /// The running result is out of range [X3.135-1986 §5.8 GR4e, GR4f].
    fn update(&mut self, v: &Value) -> Result<()>;
    /// The result for the values seen so far, possibly none.
    ///
    /// # Errors
    ///
    /// The result cannot be computed or is out of range.
    fn finish(&self) -> Result<Value>;
}

/// A shared handle to a registered scalar function.
#[derive(Clone)]
pub struct ScalarRef(pub Arc<dyn ScalarFunction>);
/// A shared handle to a registered aggregate function.
#[derive(Clone)]
pub struct AggregateRef(pub Arc<dyn AggregateFunction>);

impl fmt::Debug for ScalarRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.name())
    }
}
impl fmt::Debug for AggregateRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.name())
    }
}

/// The functions a database knows, by name: scalar and aggregate names never
/// overlap.
#[derive(Clone)]
pub struct Registry {
    scalars: HashMap<String, ScalarRef>,
    aggregates: HashMap<String, AggregateRef>,
}

impl Registry {
    /// A registry holding the five SQL-86 set functions, and the string
    /// functions of SQL-92 [ISO/IEC 9075:1992 §6.6, §6.7, §6.13].
    pub fn with_set_functions() -> Registry {
        let mut r = Registry { scalars: HashMap::new(), aggregates: HashMap::new() };
        for f in strings::functions() {
            r.scalars.insert(f.name().to_string(), ScalarRef(f));
        }
        let set_functions: Vec<Arc<dyn AggregateFunction>> =
            vec![Arc::new(Count), Arc::new(Sum), Arc::new(Avg), Arc::new(Extreme { max: true }), Arc::new(Extreme { max: false })];
        for f in set_functions {
            r.aggregates.insert(f.name().to_string(), AggregateRef(f));
        }
        r
    }

    /// Adds or replaces a scalar function.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the name is an aggregate function's: a call could not
    /// tell which is meant.
    pub fn add_scalar(&mut self, f: Arc<dyn ScalarFunction>) -> Result<()> {
        let name = f.name().to_string();
        if self.aggregates.contains_key(&name) {
            return fail(Code::Api, format!("[EXTENSION] {name} is already an aggregate function; a name cannot be both"));
        }
        self.scalars.insert(name, ScalarRef(f));
        Ok(())
    }

    /// Adds or replaces an aggregate function, including the five set functions.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the name is a scalar function's.
    pub fn add_aggregate(&mut self, f: Arc<dyn AggregateFunction>) -> Result<()> {
        let name = f.name().to_string();
        if self.scalars.contains_key(&name) {
            return fail(Code::Api, format!("[EXTENSION] {name} is already a scalar function; a name cannot be both"));
        }
        self.aggregates.insert(name, AggregateRef(f));
        Ok(())
    }

    /// The scalar function named `name`, if one is registered.
    pub fn scalar(&self, name: &str) -> Option<ScalarRef> {
        self.scalars.get(name).cloned()
    }

    /// The aggregate function named `name`, if one is registered.
    pub fn aggregate(&self, name: &str) -> Option<AggregateRef> {
        self.aggregates.get(name).cloned()
    }
}

// ---------------------------------------------------------------------------
// The SQL-86 set functions [X3.135-1986 §5.8]
// ---------------------------------------------------------------------------

/// [X3.135-1986 §5.8 SR9c] implementor-defined: AVG of an exact numeric with scale S has
/// scale S + `AVG_EXTRA_SCALE`, truncated toward zero.
pub const AVG_EXTRA_SCALE: u32 = 2;

fn not_character(name: &str, arg: Option<DataType>) -> Result<DataType> {
    match arg.filter(|t| matches!(t, DataType::Exact { .. } | DataType::Approximate { .. })) {
        Some(t) => Ok(t),
        None => fail(Code::SyntaxRule, format!("[X3.135-1986 §5.8 SR9a] the argument of {name} shall be a number")),
    }
}

/// COUNT(*) and COUNT(DISTINCT c). [X3.135-1986 §5.8 GR4a, GR4b] the cardinality.
struct Count;

impl AggregateFunction for Count {
    fn name(&self) -> &'static str {
        kw::COUNT
    }
    // [X3.135-1986 §5.8 SR7] exact numeric with implementor-defined precision and scale 0.
    fn result_type(&self, _arg: Option<DataType>) -> Result<DataType> {
        Ok(DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0 })
    }
    fn accumulator(&self, _result: DataType) -> Box<dyn Accumulator> {
        Box::new(CountAcc(0))
    }
}

struct CountAcc(i128);

impl Accumulator for CountAcc {
    fn update(&mut self, _v: &Value) -> Result<()> {
        self.0 = self.0.saturating_add(1);
        Ok(())
    }
    fn finish(&self) -> Result<Value> {
        Ok(Value::Exact(Decimal::new(self.0, 0)))
    }
}

/// SUM. [X3.135-1986 §5.8 SR9b] exact with scale S stays exact with scale S. [X3.135-1986 §5.8 GR4e] the sum
/// shall be within range. [X3.135-1986 §5.8 GR4c] null when S is empty.
struct Sum;

impl AggregateFunction for Sum {
    fn name(&self) -> &'static str {
        kw::SUM
    }
    fn result_type(&self, arg: Option<DataType>) -> Result<DataType> {
        Ok(match not_character(kw::SUM, arg)? {
            DataType::Exact { scale, .. } => DataType::Exact { precision: MAX_EXACT_PRECISION, scale },
            // [X3.135-1986 §5.8 SR9d]; `not_character` has refused CHARACTER.
            DataType::Approximate { .. }
            | DataType::Character { .. }
            | DataType::Varying { .. }
            | DataType::Bit { .. }
            | DataType::BitVarying { .. }
            | DataType::Date
            | DataType::Time { .. }
            | DataType::Timestamp { .. }
            | DataType::Extension(_) => DataType::Approximate { precision: DOUBLE_PRECISION },
        })
    }
    fn accumulator(&self, _result: DataType) -> Box<dyn Accumulator> {
        Box::new(SumAcc(Value::Null))
    }
}

struct SumAcc(Value);

impl Accumulator for SumAcc {
    fn update(&mut self, v: &Value) -> Result<()> {
        self.0 = if self.0.is_null() { v.clone() } else { arith(ArithOp::Add, &self.0, v)? };
        Ok(())
    }
    fn finish(&self) -> Result<Value> {
        Ok(self.0.clone())
    }
}

/// AVG. [X3.135-1986 §5.8 GR4f] the average; the running sum shall be within range.
struct Avg;

impl AggregateFunction for Avg {
    fn name(&self) -> &'static str {
        kw::AVG
    }
    fn result_type(&self, arg: Option<DataType>) -> Result<DataType> {
        Ok(match not_character(kw::AVG, arg)? {
            DataType::Exact { scale, .. } => {
                DataType::Exact { precision: MAX_EXACT_PRECISION, scale: scale.saturating_add(AVG_EXTRA_SCALE).min(MAX_EXACT_PRECISION) }
            }
            // `not_character` has refused CHARACTER.
            DataType::Approximate { .. }
            | DataType::Character { .. }
            | DataType::Varying { .. }
            | DataType::Bit { .. }
            | DataType::BitVarying { .. }
            | DataType::Date
            | DataType::Time { .. }
            | DataType::Timestamp { .. }
            | DataType::Extension(_) => DataType::Approximate { precision: DOUBLE_PRECISION },
        })
    }
    fn accumulator(&self, result: DataType) -> Box<dyn Accumulator> {
        Box::new(AvgAcc { sum: SumAcc(Value::Null), count: 0, result })
    }
}

struct AvgAcc {
    sum: SumAcc,
    count: i128,
    result: DataType,
}

impl Accumulator for AvgAcc {
    fn update(&mut self, v: &Value) -> Result<()> {
        self.count = self.count.saturating_add(1);
        self.sum.update(v)
    }
    fn finish(&self) -> Result<Value> {
        match (&self.sum.0, self.result) {
            (Value::Null, _) => Ok(Value::Null),
            // The sum divided by the count at the result scale.
            (Value::Exact(d), DataType::Exact { scale, .. }) => Ok(Value::Exact(d.quotient_at(Decimal::new(self.count, 0), scale)?)),
            // An approximate sum gives an approximate average.
            (sum, _) => arith(ArithOp::Div, sum, &Value::Exact(Decimal::new(self.count, 0))),
        }
    }
}

/// MAX and MIN. [X3.135-1986 §5.8 SR8] the result type is the argument's. [X3.135-1986 §5.8 GR4d] by the [X3.135-1986 §5.11] rules.
struct Extreme {
    max: bool,
}

impl AggregateFunction for Extreme {
    fn name(&self) -> &str {
        if self.max { kw::MAX } else { kw::MIN }
    }
    fn result_type(&self, arg: Option<DataType>) -> Result<DataType> {
        // [X3.135-1986 §5.8 F] only COUNT has the form (*); the extension call
        // syntax always passes an argument too.
        arg.ok_or_else(|| SqlError::new(Code::SyntaxRule, format!("[X3.135-1986 §5.8 F] {} takes an argument", self.name())))
    }
    fn accumulator(&self, _result: DataType) -> Box<dyn Accumulator> {
        Box::new(ExtremeAcc { best: Value::Null, max: self.max })
    }
}

struct ExtremeAcc {
    best: Value,
    max: bool,
}

impl Accumulator for ExtremeAcc {
    /// [X3.135-1986 §5.8 GR4d] keeps the maximum or minimum by the comparison
    /// rules of [X3.135-1986 §5.11].
    fn update(&mut self, v: &Value) -> Result<()> {
        let better = match compare(v, &self.best) {
            None => true, // nothing seen yet
            Some(o) => o == if self.max { std::cmp::Ordering::Greater } else { std::cmp::Ordering::Less },
        };
        if better {
            self.best = v.clone();
        }
        Ok(())
    }
    fn finish(&self) -> Result<Value> {
        Ok(self.best.clone())
    }
}

// Unit tests: what the set functions refuse and how they total (RULES.md
// R-TEST-2 robustness).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [X3.135-1986 §5.8 F] MAX and MIN take an argument; asked for
    /// the type of an argumentless call, they refuse rather than panic.
    #[test]
    fn max_and_min_need_an_argument() {
        for max in [true, false] {
            assert_eq!(Extreme { max }.result_type(None).err().map(|e| e.code), Some(Code::SyntaxRule));
        }
    }

    /// Verifies: [X3.135-1986 §5.8 GR4f] AVG of approximate values is approximate.
    #[test]
    fn approximate_average() {
        let mut avg = Avg.accumulator(DataType::Approximate { precision: DOUBLE_PRECISION });
        for x in [1.0_f64, 2.0_f64] {
            avg.update(&Value::Approx(x)).unwrap();
        }
        assert!(matches!(avg.finish(), Ok(Value::Approx(a)) if (a - 1.5_f64).abs() < f64::EPSILON));
    }
}
