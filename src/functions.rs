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

use std::fmt;
use std::sync::Arc;

use crate::error::{Code, Result, fail};
use crate::json::Warned;
use crate::lexer::kw;
use crate::params::ParameterType;
use crate::storage::{Floats, Ints, Texts};
use crate::value::Spelled;
use crate::value::{DOUBLE_PRECISION, DataType, Decimal, MAX_DECFLOAT_PRECISION, MAX_EXACT_PRECISION, Value};

mod any_value;
pub use any_value::ANY_VALUE;
pub(crate) use any_value::GroupValue;
mod arrays;
pub use arrays::{
    ARRAY_AGG, ARRAY_MAX_CARDINALITY, ArrayAggregate, ArrayConcatenation, ArrayOf, AssignElement, CARDINALITY, CardinalityOf, ElementOf,
    TRIM_ARRAY, TrimArrayOf,
};
mod distribution;
pub use distribution::{FRACTION, InverseDistribution, PERCENTILE_CONT, PERCENTILE_DISC, Picked, picked, picked_value};
mod extremes;
pub use extremes::CollatedExtreme;
use extremes::Extreme;
mod floats;
mod formatted;
pub(crate) use formatted::json_datetime_cast;
pub use formatted::{FORMAT, formatted_cast};
mod greatest;
pub use greatest::{GREATEST, GreatestOrLeast, LEAST};
mod hypothetical;
pub use hypothetical::{Hypothesis, Hypothetical, RANK_FUNCTION_TYPES};
mod ints;
mod listagg;
mod multisets;
pub use multisets::{
    COLLECT, ELEMENT, ElementOfMultiset, FUSION, Gathering, INTERSECTION, IsASet, MEMBER, MemberOf, MultisetAggregate, MultisetCombination,
    MultisetOf, SUBMULTISET, SetOf, SubmultisetOf,
};
mod octet_like;
mod periods;
pub use octet_like::OctetLike;
pub use periods::PeriodTest;
pub mod lobs;
pub use listagg::{LISTAGG, Listagg, ListaggSpec, MAX_LISTAGG_LENGTH, Truncation};
mod json;
pub use json::{JSON_EXISTS, JSON_QUERY, JSON_VALUE, JsonCall, JsonOperation, ValueSlot};
mod regex;
mod registry;
mod rows;
pub use rows::{FieldOf, RowOf};
mod strings;
mod structured;
pub use structured::{
    Constructed, Instanced, Keyed, Mutator, Observer, SPECIFICTYPE, SPECIFICTYPE_LENGTH, SpecificType, Treat, TreatReference, TypeTest,
};
mod sum;
mod trim;
mod truths;
mod width_bucket;
use sum::{AvgAcc, SumAcc};

pub use regex::{
    AFTER, CHARACTERS, FLAG, LIKE_REGEX, LikeRegex, OCCURRENCE, OCCURRENCES_REGEX, OCTETS, POSITION_REGEX, REGEX_FUNCTIONS, START,
    SUBSTRING_REGEX, TRANSLATE_REGEX, regex_function, subject_of,
};
pub use registry::Registry;
pub(crate) use registry::Standard;
pub use strings::{CONCATENATION, OVERLAY, PLACING, using};
pub use trim::{BTRIM, LPAD, LTRIM, RPAD, RTRIM, TRIMS_AND_PADS, Trim, source_of, trim_named};
pub use truths::EVERY;
use truths::Truths;
pub use width_bucket::{WIDTH_BUCKET, width_bucket};

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
/// * Of a name, several functions are registered only if each declares the
///   types of its parameters (`parameter_types`) and no call could have
///   two of them as its subject (RULES.md D74).
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
    /// \[DESIGN\] As `call`, given where the database keeps temporary
    /// large objects (RULES.md D79), for a function making a large object
    /// longer than a chunk; none where no database runs it. Default:
    /// `call`.
    ///
    /// # Errors
    ///
    /// As `call`.
    fn call_with(&self, args: &[Value], _temp: Option<&dyn crate::value::lob::TempLobs>) -> Result<Value> {
        self.call(args)
    }
    /// As `call_with`, with borrowed arguments and the same temporary
    /// large-object store. Implementations may avoid
    /// copying large values or changing shared reference counts. Results,
    /// errors and side effects must be identical to `call_with`; arguments
    /// cannot be retained beyond this call without taking ownership.
    /// The default copies arguments and calls `call_with` exactly once.
    ///
    /// # Errors
    ///
    /// The same errors as `call_with` for the same arguments.
    fn call_borrowed(&self, args: &[&Value], temp: Option<&dyn crate::value::lob::TempLobs>) -> Result<Value> {
        match args {
            [] => self.call_with(&[], temp),
            [one] => self.call_with(&[(*one).clone()], temp),
            [left, right] => self.call_with(&[(*left).clone(), (*right).clone()], temp),
            _ => self.call_with(&args.iter().map(|value| (*value).clone()).collect::<Vec<_>>(), temp),
        }
    }

    /// \[DESIGN\] As `call_with`, and the completion condition the call
    /// raised, a warning [ISO/IEC 9075:1992 §3.3.4.1], if one: as a cast
    /// within it raises one, which the statement completes with, as of
    /// `Expr::Cast`. Default: `call_with`, raising none.
    ///
    /// # Errors
    ///
    /// As `call`.
    fn call_warned(&self, args: &[Value], temp: Option<&dyn crate::value::lob::TempLobs>) -> Result<Warned> {
        self.call_with(args, temp).map(|value| (value, None))
    }
    /// \[DESIGN\] Whether a call may raise a warning, so that it is made
    /// by `call_warned` and not by the calls that borrow their arguments
    /// (`exec/eval.rs`). Default: no.
    fn warns(&self) -> bool {
        false
    }
    /// \[EXTENSION\] Whether an index may find rows for this function in
    /// a WHERE filter, such as `f(column, ?)` (RULES.md D34).
    /// True promises that every call accepted by the function's type
    /// checks returns Boolean or null without an error or side effect.
    /// Equal inputs give equal answers; a null argument gives null.
    /// `deterministic` must also return true. These promises hold for the
    /// function's life. Argument expressions are checked separately:
    /// an expression that can raise still requires full evaluation (D15).
    /// Defaults to false, so existing functions do not opt in.
    fn index_predicate(&self) -> bool {
        false
    }
    /// A spatial operation whose complete behavior this function implements,
    /// including result types, NULLs and SRID errors (D85). Deterministic,
    /// side-effect-free, and fixed for its lifetime. Unlike `index_predicate`,
    /// this does not promise error-free calls: pruning requires a separate
    /// proof and a runtime fallback for changed prepared arguments.
    fn spatial_operation(&self) -> Option<crate::geometry::SpatialOperation> {
        None
    }
    /// Same arguments always give the same result? If so, the optimizer may
    /// evaluate calls with constant arguments once, at planning time.
    fn deterministic(&self) -> bool {
        true
    }
    /// Prepares an equivalent implementation using known constant
    /// arguments; `None` marks an argument that varies. Called by constant
    /// folding, without evaluating varying arguments. Retained prepared
    /// state draws on `budget`; decline if it cannot be reserved.
    /// Default: no change.
    ///
    /// The returned function must preserve the name, type contracts,
    /// determinism, index capabilities, results and runtime errors of this
    /// function. Preparation must have no side effects or raise errors:
    /// leave a failing preparation for `call`, which may never be reached.
    /// It must remain correct for different arguments too, since plans
    /// may be reused. Return `None` from an already prepared implementation
    /// to let optimization reach a fixed point.
    fn specialize(&self, _args: &[Option<&Value>], _budget: &Arc<crate::memory::MemoryBudget>) -> Option<ScalarRef> {
        None
    }
    /// \[EXTENSION\] The type of argument `position`, from 0, if the
    /// function declares one, the arguments' own types given in `args`,
    /// none for a `<dynamic parameter specification>`. An argument of
    /// another type is stored in it by store assignment [ISO/IEC 9075:1992
    /// §9.2] before `call` sees it, so a character string reaches a type
    /// beyond the standard that takes one (`SqlType::assigns_text`, RULES.md
    /// D53); one that cannot be \[SR2\] is refused as the statement is
    /// bound (SQLCODE -103). By default the type of its parameter there, if
    /// it declares them (`parameter_types`); else none: each argument is
    /// given as it is, its type checked by `return_type` alone.
    fn argument_type(&self, position: usize, _args: &[Option<DataType>]) -> Option<DataType> {
        self.parameter_types().and_then(|types| types.into_iter().nth(position))
    }
    /// \[EXTENSION\] [ISO/IEC 9075-2:2023 §9.18 SR10] The types of its
    /// parameters, if it declares them: it takes as many arguments, each
    /// of a type whose precedence list holds its parameter's designator
    /// [§9.7], stored in it as by `argument_type`; and of the functions of
    /// its name registered, a call invokes the one its arguments' types
    /// choose [§9.6] (RULES.md D74). By default none: it is alone of its
    /// name, its arguments' types checked by `return_type`.
    fn parameter_types(&self) -> Option<Vec<DataType>> {
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
    /// \[DESIGN\] What the label of its call shows after its argument, so
    /// two calls of it of one argument that differ otherwise, as by the
    /// directions of their sort keys, are computed apart. Default: nothing.
    fn label_suffix(&self) -> String {
        String::new()
    }
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
    /// \[DESIGN\] Appends its state so far to `out`, as bytes `merge` takes
    /// in another accumulator of its function and result type (RULES.md
    /// D38, D53): a grouping past its work area then writes groups partly
    /// made, not their rows. Whether it has one: by default it has none,
    /// writes nothing, and its rows are written, so it is given them all.
    fn write_state(&self, _out: &mut Vec<u8>) -> bool {
        false
    }
    /// \[DESIGN\] Takes in `state`, as `write_state` wrote it of an accumulator
    /// of its function and result type given the values after those it
    /// was given, and before any it is given next: as if it had been given
    /// them. It judges no result: `finish` does.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if `state` is not one it writes; by default, as it
    /// writes none.
    fn merge(&mut self, _state: &[u8]) -> Result<()> {
        fail(Code::Corrupt, "[DESIGN] a set function's state that it never writes")
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

// ---------------------------------------------------------------------------
// The SQL-86 set functions [X3.135-1986 §5.8]
// ---------------------------------------------------------------------------

/// [X3.135-1986 §5.8 SR9c] implementor-defined: AVG of an exact numeric with scale S has
/// scale S + `AVG_EXTRA_SCALE`, truncated toward zero.
pub const AVG_EXTRA_SCALE: u32 = 2;

fn not_character(name: &str, arg: Option<DataType>) -> Result<DataType> {
    match arg.filter(|t| t.is_numeric() || matches!(t, DataType::Interval(_))) {
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
    /// The count, its 16 bytes.
    fn write_state(&self, out: &mut Vec<u8>) -> bool {
        out.extend_from_slice(&self.0.to_le_bytes());
        true
    }
    fn merge(&mut self, state: &[u8]) -> Result<()> {
        let n = <[u8; 16]>::try_from(state).map(i128::from_le_bytes);
        self.0 = self.0.saturating_add(n.or_else(|_| fail(Code::Corrupt, "[DESIGN] a state of COUNT is not 16 bytes"))?);
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
            // [ISO/IEC 9075-2:2023 §10.9 SR7g v] DECFLOAT of 34 digits (IV170).
            DataType::DecFloat { .. } => DataType::DecFloat { precision: MAX_DECFLOAT_PRECISION },
            // [X3.135-1986 §5.8 SR9d]; `not_character` has refused the types not
            // numbers or intervals.
            DataType::Approximate { .. }
            | DataType::Character { .. }
            | DataType::Varying { .. }
            | DataType::Binary { .. }
            | DataType::VarBinary { .. }
            | DataType::Lob { .. }
            | DataType::Boolean
            | DataType::Date
            | DataType::Time { .. }
            | DataType::Timestamp { .. }
            | DataType::Array(_)
            | DataType::Multiset(_)
            | DataType::Structured(_)
            | DataType::Reference(_)
            | DataType::Row(_)
            | DataType::Distinct(_)
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
            // [ISO/IEC 9075-2:2023 §10.9 SR7g v] DECFLOAT of 34 digits (IV170).
            DataType::DecFloat { .. } => DataType::DecFloat { precision: MAX_DECFLOAT_PRECISION },
            // `not_character` has refused the types not numbers or intervals.
            DataType::Approximate { .. }
            | DataType::Character { .. }
            | DataType::Varying { .. }
            | DataType::Binary { .. }
            | DataType::VarBinary { .. }
            | DataType::Lob { .. }
            | DataType::Boolean
            | DataType::Date
            | DataType::Time { .. }
            | DataType::Timestamp { .. }
            | DataType::Array(_)
            | DataType::Multiset(_)
            | DataType::Structured(_)
            | DataType::Reference(_)
            | DataType::Row(_)
            | DataType::Distinct(_)
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
mod borrowed;
#[cfg(test)]
mod tests;
