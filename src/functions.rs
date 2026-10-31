//! Functions as plugins.
//!
//! SQL-86 has exactly five set functions [X3.135-1986 §5.8], and their names are key
//! words: COUNT, SUM, AVG, MAX, MIN. They are implemented here as ordinary
//! `AggregateFunction` plugins registered under those names, so an
//! implementation can be swapped (say, a more accurate SUM) without touching
//! the parser, planner or executor. So are SQL-92's string functions and
//! concatenation (`functions/strings.rs`).
//!
//! \[EXTENSION\] SQL-86 has no syntax for calling any other function. As an opt-in,
//! non-standard extension ([X3.135-1986 §3.4] allows "user options to process nonconforming
//! SQL language"), a session whose SQL.FUNCTION_CALLS is ON accepts `<identifier>`(`<args>`) and resolves it
//! to a registered `ScalarFunction` or `AggregateFunction`.
//!
//! The engine, not the plugin, applies [X3.135-1986 §5.8 GR1, GR2]: nulls are removed from
//! the argument, and duplicates too for DISTINCT, before `update` is called.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use crate::error::{Code, Result, fail};
use crate::lexer::kw;
use crate::params::ParameterType;
use crate::storage::{Floats, Ints, Texts};
use crate::value::Spelled;
use crate::value::{DOUBLE_PRECISION, DataType, Decimal, MAX_EXACT_PRECISION, Value};

mod extremes;
use extremes::Extreme;
mod floats;
mod ints;
mod strings;
mod sum;
use sum::{AvgAcc, SumAcc};

pub use strings::CONCATENATION;

/// \[EXTENSION\] A function callable with the opt-in call syntax.
///
/// Contract (RULES.md R-SAFE-3):
/// * `name` is an upper-case `<identifier>`, not an aggregate's.
/// * `return_type` is given the type of each argument: the type
///   `argument_type` declares for it, if it declares one, else the
///   argument's own.
/// * `call` is given each argument as `return_type` was told its type: an
///   argument of a type `argument_type` declares is assigned to it first.
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
    /// \[DESIGN\] Itself as it declares settings and is told them
    /// (`Configurable`, RULES.md D36), if it has any. Default: none.
    fn configurable(&self) -> Option<&dyn crate::settings::Configurable> {
        None
    }
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
    /// \[EXTENSION\] The type of argument `position`, from 0, if the
    /// function declares one, the arguments' own types given in `args`,
    /// none for a `<dynamic parameter specification>`. An argument of
    /// another type is stored in it by store assignment [ISO/IEC 9075:1992
    /// §9.2] before `call` sees it, so a character string reaches a type
    /// beyond the standard that takes one (`SqlType::assigns_text`, RULES.md
    /// D53); one that cannot be \[SR2\] is refused as the statement is
    /// bound (SQLCODE -103). By default none: each argument is given as it
    /// is, its type checked by `return_type` alone.
    fn argument_type(&self, _position: usize, _args: &[Option<DataType>]) -> Option<DataType> {
        None
    }
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §17.6 GR5] The type a `<dynamic
    /// parameter specification>` takes as argument `position`, from 0, the
    /// other arguments' types given in `args`, none for each that is itself
    /// one; none if it takes none there, where it is refused \[GR4\]. By
    /// default the type `argument_type` declares there, if it declares one.
    fn parameter_type(&self, position: usize, args: &[Option<DataType>]) -> Option<ParameterType> {
        self.argument_type(position, args).map(ParameterType::Of)
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
    /// \[DESIGN\] Itself as it declares settings and is told them
    /// (`Configurable`, RULES.md D36), if it has any. Default: none.
    fn configurable(&self) -> Option<&dyn crate::settings::Configurable> {
        None
    }
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
    /// The value cannot be taken in. The built-in SUM and AVG raise here
    /// only of a sum 128 bits cannot hold, a limit: a result out of range is
    /// raised by `finish` [ISO/IEC 9075:1992 §6.5 GR2b iv], so whether it
    /// is does not depend on the order of the values (RULES.md D15).
    fn update(&mut self, v: &Value) -> Result<()>;
    /// \[DESIGN\] Called with a vector of the argument's values (RULES.md
    /// D31): of those at `rows`, or all, each not null, as `update` is.
    /// Default: `update` of each.
    ///
    /// # Errors
    ///
    /// As `update`.
    fn update_many(&mut self, values: &[Value], rows: Option<&[usize]>) -> Result<()> {
        match rows {
            Some(rows) => rows.iter().filter_map(|&r| values.get(r)).filter(|v| !v.is_null()).try_for_each(|v| self.update(v)),
            None => values.iter().filter(|v| !v.is_null()).try_for_each(|v| self.update(v)),
        }
    }
    /// \[DESIGN\] Called with a vector of exact numbers as integers
    /// (`Ints`, RULES.md D31): of those at `rows`, or all, nulls left out,
    /// as `update_many` is. Default: `update_many` of their values.
    ///
    /// # Errors
    ///
    /// As `update`.
    fn update_ints(&mut self, ints: &Ints, rows: Option<&[usize]>) -> Result<()> {
        self.update_many(ints.values(), rows)
    }
    /// \[DESIGN\] Called with a vector of approximate numbers (`Floats`,
    /// RULES.md D31), as `update_many` is. Default: `update_many` of their
    /// values.
    ///
    /// # Errors
    ///
    /// As `update`.
    fn update_floats(&mut self, floats: &Floats, rows: Option<&[usize]>) -> Result<()> {
        self.update_many(floats.values(), rows)
    }
    /// \[DESIGN\] Called with a vector of character strings as their text
    /// (`Texts`, RULES.md D31), as `update_many` is. Default: `update_many`
    /// of their values.
    ///
    /// # Errors
    ///
    /// As `update`.
    fn update_texts(&mut self, texts: &Texts, rows: Option<&[usize]>) -> Result<()> {
        self.update_many(texts.values(), rows)
    }
    /// \[DESIGN\] For COUNT(*): called once for `rows` rows, as `update`
    /// is once a row. Default: `update` of a null placeholder for each.
    ///
    /// # Errors
    ///
    /// As `update`.
    fn update_rows(&mut self, rows: usize) -> Result<()> {
        (0..rows).try_for_each(|_| self.update(&Value::Null))
    }
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
    /// The standard COUNT, MAX and MIN, in `Standard`'s order, which a
    /// plug-in of its name may replace.
    standard: [AggregateRef; 3],
}

/// A set function a `Registry` holds as the standard defines it, no
/// plug-in of its name [ISO/IEC 9075:1992 §6.5].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Standard {
    /// COUNT.
    Count,
    /// MAX.
    Max,
    /// MIN.
    Min,
}

impl Registry {
    /// A registry holding the five SQL-86 set functions, and the string
    /// functions of SQL-92 [ISO/IEC 9075:1992 §6.6, §6.7, §6.13].
    pub fn with_set_functions() -> Registry {
        let standard =
            [AggregateRef(Arc::new(Count)), AggregateRef(Arc::new(Extreme { max: true })), AggregateRef(Arc::new(Extreme { max: false }))];
        let scalars = strings::functions().into_iter().map(|f| (f.name().to_string(), ScalarRef(f))).collect();
        let (sum, avg): (Arc<dyn AggregateFunction>, Arc<dyn AggregateFunction>) = (Arc::new(Sum), Arc::new(Avg));
        let set_functions = standard.iter().map(|f| Arc::clone(&f.0)).chain([sum, avg]);
        let aggregates = set_functions.map(|f| (f.name().to_string(), AggregateRef(f))).collect();
        Registry { scalars, aggregates, standard }
    }

    /// Adds a scalar function, one for each name (RULES.md D53).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the name is an aggregate function's (a call could not
    /// tell which is meant), or a scalar function's, a standard one's too.
    pub fn add_scalar(&mut self, f: Arc<dyn ScalarFunction>) -> Result<()> {
        let name = f.name().to_string();
        if self.aggregates.contains_key(&name) {
            return fail(Code::Api, format!("[EXTENSION] {name} is already an aggregate function; a name cannot be both"));
        }
        if self.scalars.contains_key(&name) {
            return fail(Code::Api, format!("[EXTENSION] {name} is already a scalar function, and is not replaced"));
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

    /// Each function registered that declares settings (RULES.md D36).
    pub(crate) fn configurables(&self) -> impl Iterator<Item = &dyn crate::settings::Configurable> {
        let scalars = self.scalars.values().filter_map(|f| f.0.configurable());
        scalars.chain(self.aggregates.values().filter_map(|f| f.0.configurable()))
    }

    /// The scalar function named `name`, if one is registered.
    pub fn scalar(&self, name: &str) -> Option<ScalarRef> {
        self.scalars.get(name).cloned()
    }

    /// The aggregate function named `name`, if one is registered.
    pub fn aggregate(&self, name: &str) -> Option<AggregateRef> {
        self.aggregates.get(name).cloned()
    }

    /// Which of the standard COUNT, MAX and MIN [X3.135-1986 §5.8] `f` is,
    /// if it is one, no plug-in of its name.
    pub(crate) fn standard(&self, f: &AggregateRef) -> Option<Standard> {
        let held = |at: usize| self.standard.get(at).is_some_and(|s| std::ptr::addr_eq(Arc::as_ptr(&f.0), Arc::as_ptr(&s.0)));
        [Standard::Count, Standard::Max, Standard::Min].into_iter().enumerate().find_map(|(at, s)| held(at).then_some(s))
    }
}

// ---------------------------------------------------------------------------
// The SQL-86 set functions [X3.135-1986 §5.8]
// ---------------------------------------------------------------------------

/// [X3.135-1986 §5.8 SR9c] implementor-defined: AVG of an exact numeric with scale S has
/// scale S + `AVG_EXTRA_SCALE`, truncated toward zero.
pub const AVG_EXTRA_SCALE: u32 = 2;

fn not_character(name: &str, arg: Option<DataType>) -> Result<DataType> {
    match arg.filter(|t| matches!(t, DataType::Exact { .. } | DataType::Approximate { .. } | DataType::Interval(_))) {
        Some(t) => Ok(t),
        None => fail(
            Code::SyntaxRule,
            format!("[X3.135-1986 §5.8 SR9a; ISO/IEC 9075:1992 §6.5 SR9a] the argument of {name} shall be a number or an interval"),
        ),
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
        Ok(DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric })
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
    fn update_many(&mut self, values: &[Value], rows: Option<&[usize]>) -> Result<()> {
        let counted = match rows {
            Some(rows) => rows.iter().filter_map(|&r| values.get(r)).filter(|v| !v.is_null()).count(),
            None => values.iter().filter(|v| !v.is_null()).count(),
        };
        self.update_rows(counted)
    }
    fn update_ints(&mut self, ints: &Ints, rows: Option<&[usize]>) -> Result<()> {
        self.update_rows(ints::count(ints, rows))
    }
    fn update_floats(&mut self, floats: &Floats, rows: Option<&[usize]>) -> Result<()> {
        self.update_rows(floats::count(floats, rows))
    }
    /// Its texts counted, none made a value.
    fn update_texts(&mut self, texts: &Texts, rows: Option<&[usize]>) -> Result<()> {
        let counted = match rows {
            Some(rows) => rows.iter().filter(|&&n| texts.get(n).is_some()).count(),
            None => (0..texts.len()).filter(|&n| texts.get(n).is_some()).count(),
        };
        self.update_rows(counted)
    }
    fn update_rows(&mut self, rows: usize) -> Result<()> {
        self.0 = self.0.saturating_add(i128::try_from(rows).unwrap_or(i128::MAX));
        Ok(())
    }
    fn finish(&self) -> Result<Value> {
        Ok(Value::Exact(Decimal::new(self.0, 0)))
    }
}

/// SUM. [X3.135-1986 §5.8 SR9b] exact with scale S stays exact with scale S. [ISO/IEC 9075:1992 §6.5 GR2b iv]
/// the sum, not each sum on the way to it, shall be within range (`sum.rs`). [X3.135-1986 §5.8 GR4c] null when S is
/// empty.
struct Sum;

impl AggregateFunction for Sum {
    fn name(&self) -> &'static str {
        kw::SUM
    }
    fn result_type(&self, arg: Option<DataType>) -> Result<DataType> {
        Ok(match not_character(kw::SUM, arg)? {
            DataType::Exact { scale, .. } => DataType::Exact { precision: MAX_EXACT_PRECISION, scale, spelled: Spelled::Numeric },
            // [ISO/IEC 9075:1992 §6.5 SR9e] of the interval's precision.
            DataType::Interval(q) => DataType::Interval(q),
            // [X3.135-1986 §5.8 SR9d]; `not_character` has refused the types not
            // numbers or intervals.
            DataType::Approximate { .. }
            | DataType::Character { .. }
            | DataType::Varying { .. }
            | DataType::Bit { .. }
            | DataType::BitVarying { .. }
            | DataType::Date
            | DataType::Time { .. }
            | DataType::Timestamp { .. }
            | DataType::Extension(_) => DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::Float },
        })
    }
    fn accumulator(&self, result: DataType) -> Box<dyn Accumulator> {
        Box::new(SumAcc::new(result))
    }
}

/// AVG. [ISO/IEC 9075:1992 §6.5 GR2b ii] the average, which alone shall be within range (`sum.rs`): SQL-92 drops
/// the range of the sum that [X3.135-1986 §5.8 GR4f] sets (RULES.md D32).
struct Avg;

impl AggregateFunction for Avg {
    fn name(&self) -> &'static str {
        kw::AVG
    }
    fn result_type(&self, arg: Option<DataType>) -> Result<DataType> {
        Ok(match not_character(kw::AVG, arg)? {
            DataType::Exact { scale, .. } => DataType::Exact {
                precision: MAX_EXACT_PRECISION,
                scale: scale.saturating_add(AVG_EXTRA_SCALE).min(MAX_EXACT_PRECISION),
                spelled: Spelled::Numeric,
            },
            // [ISO/IEC 9075:1992 §6.5 SR9e] of the interval's precision.
            DataType::Interval(q) => DataType::Interval(q),
            // `not_character` has refused the types not numbers or intervals.
            DataType::Approximate { .. }
            | DataType::Character { .. }
            | DataType::Varying { .. }
            | DataType::Bit { .. }
            | DataType::BitVarying { .. }
            | DataType::Date
            | DataType::Time { .. }
            | DataType::Timestamp { .. }
            | DataType::Extension(_) => DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::Float },
        })
    }
    fn accumulator(&self, result: DataType) -> Box<dyn Accumulator> {
        Box::new(AvgAcc::new(result))
    }
}

// Unit tests: what the set functions refuse and how they total (RULES.md
// R-TEST-2 robustness).
#[cfg(test)]
mod tests;
