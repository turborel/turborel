//! Functions as plugins.
//!
//! SQL-86 has exactly five set functions [X3.135-1986 §5.8], and their names are key words:
//! COUNT, SUM, AVG, MAX, MIN. They are implemented here as ordinary `AggregateFunction`
//! plugins registered under those names, so an implementation can be swapped (say, a more
//! accurate SUM) without touching the parser, planner or executor. So are SQL-92's string
//! functions and concatenation (`functions/strings.rs`).
//!
//! \[EXTENSION\] A function registered belongs to no schema, so a call of it is no
//! `<routine invocation>` [ISO/IEC 9075-2:2023 §10.4]. A session whose SQL.FUNCTION_CALLS is
//! ON, as it is by default, resolves `<identifier>`(`<args>`) that names no function of the
//! schema to a registered `ScalarFunction` or `AggregateFunction`; OFF, it takes the schema's
//! own functions alone ([X3.135-1986 §3.4] allows "user options to process nonconforming SQL language").
//!
//! The engine, not the plugin, applies [X3.135-1986 §5.8 GR1, GR2]: nulls are removed from
//! the argument, and duplicates too for DISTINCT, before `update` is called.

use std::sync::Arc;

use crate::error::Result;
use crate::json::Warned;
use crate::params::ParameterType;
use crate::storage::Column;
use crate::value::{CompOp, DataType, Value};

mod accumulator;
pub use accumulator::Accumulator;
mod columnar;
pub use columnar::Columnar;
pub(crate) use columnar::{
    Kind as ColumnarKind, NO_FLOATS, Summed, avg_acc, avg_finished, count_acc, count_finished, extreme_acc, plus_float, sum_acc,
    sum_finished,
};
pub(crate) use floats::Wide;
mod any_value;
pub use any_value::ANY_VALUE;
pub(crate) use any_value::GroupValue;
mod arrays;
pub mod builtin;
pub use arrays::{
    ARRAY, ARRAY_AGG, ARRAY_ELEMENT, ARRAY_MAX_CARDINALITY, ArrayAggregate, ArrayConcatenation, ArrayOf, ArrayOperation, AssignElement,
    CARDINALITY, CardinalityOf, ElementOf, TRIM_ARRAY, TrimArrayOf,
};
mod context;
mod count;
pub use context::{AppendedQuery, CallContext};
use count::Count;
mod distribution;
pub use distribution::{FRACTION, InverseDistribution, PERCENTILE_CONT, PERCENTILE_DISC, Picked, picked, picked_value};
mod extremes;
mod family;
pub use extremes::CollatedExtreme;
use extremes::Extreme;
pub use family::{FamilySpecificType, FamilyTreat, FamilyTypeTest, below};
mod floats;
mod formatted;
pub(crate) use formatted::json_datetime_cast;
pub use formatted::{FORMAT, formatted_cast};
mod greatest;
mod handles;
pub use greatest::{GREATEST, GreatestOrLeast, LEAST};
pub use handles::{AggregateRef, ScalarRef};
mod hypothetical;
pub use hypothetical::{Hypothesis, Hypothetical, RANK_FUNCTION_TYPES};
mod ints;
mod listagg;
mod multisets;
mod numeric;
pub use multisets::{
    COLLECT, ELEMENT, ElementOfMultiset, FUSION, Gathering, INTERSECTION, IsASet, MEMBER, MULTISET, MemberOf, MultisetAggregate,
    MultisetCombination, MultisetOf, SUBMULTISET, SetOf, SubmultisetOf,
};
pub use numeric::{
    ABS, ACOS, ASIN, ATAN, CEIL, CEILING, COS, COSH, EXP, FLOOR, LN, LOG, LOG10, MOD, Numeric, POWER, SIN, SINH, SQRT, TAN, TANH, Trig,
};
mod octet_like;
pub(crate) mod over_vectors;
mod partial;
#[cfg(test)]
pub(crate) use partial::MERGED;
pub(crate) use partial::{Merged, PART_TYPE, PARTIAL, Partial, Split, split_of};
mod periods;
pub use octet_like::OctetLike;
pub use periods::PeriodTest;
pub mod lobs;
pub use listagg::{LISTAGG, Listagg, ListaggSpec, MAX_LISTAGG_LENGTH, Truncation};
mod json;
pub use json::{
    Construction, IsJsonCall, JSON_ARRAY, JSON_ARRAYAGG, JSON_EXISTS, JSON_OBJECT, JSON_OBJECTAGG, JSON_QUERY, JSON_VALUE,
    JsonArrayAggregate, JsonCall, JsonConstruct, JsonObjectAggregate, JsonOperation, ValueSlot,
};
pub use json::{JSON_CONTAINS, JsonContains, JsonOperator, JsonProbe, OnError};
pub(crate) use json::{JsonCastFails, JsonValueTaken, OPERANDS_PER_MEMBER, TAKEN_EMPTY, TAKEN_VALUE};
pub(crate) use json::{input_type, json_text, name_type, output_type};
mod regex;
mod registry;
mod rows;
mod scn;
mod statistics;
pub use rows::{FieldOf, RowOf};
mod strings;
pub use statistics::{PAIRED, Statistic, paired};
mod structured;
pub use structured::{
    Constructed, Instanced, Keyed, Mutator, Observer, SPECIFICTYPE, SPECIFICTYPE_LENGTH, SpecificType, Treat, TreatReference, TypeTest,
};
mod sum;
mod totals;
pub use totals::AVG_EXTRA_SCALE;
use totals::{Avg, Sum};
mod trim;
mod truths;
mod width_bucket;
mod xml;

pub use regex::{
    AFTER, CHARACTERS, FLAG, LIKE_REGEX, LikeRegex, OCCURRENCE, OCCURRENCES_REGEX, OCTETS, POSITION_REGEX, REGEX_FUNCTIONS, START,
    SUBSTRING_REGEX, TRANSLATE_REGEX, regex_function, subject_of,
};
pub use registry::Registry;
pub(crate) use registry::Standard;
pub use scn::{SCN_TO_TIMESTAMP, TIMESTAMP_TO_SCN};
pub use strings::{CONCATENATION, OVERLAY, PLACING, using};
pub use trim::{BTRIM, LPAD, LTRIM, RPAD, RTRIM, TRIMS_AND_PADS, Trim, source_of, trim_named};
pub use truths::EVERY;
use truths::Truths;
pub use width_bucket::{WIDTH_BUCKET, width_bucket};
/// [ISO/IEC 9075-14:2006 draft §9.4 to §9.7] SQL data types mapped to
/// XML Schema data types (`functions/xml/map.rs`).
pub use xml::map as xml_map;
pub use xml::{According, Candidate, ValidBy, XmlValid, XmlValidate};
pub use xml::{
    BinaryEncoding, IMPLICIT_BINARY_ENCODING, Mapping, Serialization, XMLAGG, XmlAggregate, XmlCastIn, XmlCastOut, XmlComment, XmlConcat,
    XmlCopy, XmlDocument, XmlElement, XmlForest, XmlIs, XmlParse, XmlPi, XmlSerialize, XmlText, xml_lexical, xml_mapped,
    xml_schema_restriction, xml_schema_type, xml_text,
};
pub use xml::{Evaluation, RegularColumn, TableColumn, XmlExistsOf, XmlFilter, XmlQueryOf, XmlRowsOf, XmlTableOf};
pub use xml::{IDENTIFIER_TO_XMLNAME, IDENTIFIER_TO_XMLNAME_FULLY, XMLNAME_TO_IDENTIFIER};

/// \[DESIGN\] An argument of a call over vectors (RULES.md D363,
/// `ScalarFunction::eval_batch`): a vector of a value a row, or a
/// constant, every row's.
#[derive(Debug, Clone, Copy)]
pub enum Arg<'a> {
    /// A vector of a value a row.
    Column(&'a Column),
    /// A constant.
    Const(&'a Value),
}

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
/// * `eval_batch`, if it gives a vector, gives each row the value `call`
///   gives of that row's arguments, of the kind any vector may be; it
///   gives an error only if some row's `call` would raise one, which is
///   then raised by calling the rows one by one (RULES.md D363).
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
    /// \[DESIGN\] Whether a call of it may run long, so the statement
    /// running it passes itself, to be cancelled as the call goes
    /// (`call_in_statement`, RULES.md D44): of an XQuery evaluation.
    /// Default: no.
    fn in_statement(&self) -> bool {
        false
    }
    /// \[DESIGN\] As `call_with`, given the statement that runs it, which
    /// says whether it was cancelled (`Subqueries::interrupted`) and
    /// evaluates expressions of the function's own: a statement calls this
    /// of a function that is `in_statement`. Default: `call_with`.
    ///
    /// # Errors
    ///
    /// As `call_with`, or the statement's cancellation.
    fn call_in_statement(&self, args: &[Value], statement: &dyn crate::exec::Subqueries) -> Result<Value> {
        self.call_with(args, statement.temp_lobs())
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
    /// \[EXTENSION\] RULES.md D106: what this function's value says of
    /// whether a query's value is empty, if it does, so an XML index may
    /// find the rows it keeps (`xml::XmlFilter`): XMLEXISTS, XMLQUERY NULL
    /// ON EMPTY, XMLTABLE's row pattern, or a function null exactly when
    /// its argument is. Default: none.
    fn xml_filter(&self) -> Option<XmlFilter> {
        None
    }
    /// \[DESIGN\] Whether no call of it raises an error, of any arguments
    /// its type checks accept, but the statement's cancellation, so a
    /// conjunct of it need not see every row (RULES.md D15): of XMLEXISTS
    /// of a query that raises none (`xquery::Reduction::raise_free`).
    /// Default: no.
    fn raise_free(&self) -> bool {
        false
    }
    /// \[EXTENSION\] Whether a call of `args` may be part of an index's key
    /// (`index/expression.rs`, RULES.md D92): true promises that
    /// it raises no exception condition for any values of their types,
    /// those constant being these, and gives one value of equal ones,
    /// whatever its session. False by default.
    fn index_safe(&self, _args: &[crate::index::expression::KeyArgument<'_>]) -> bool {
        false
    }
    /// \[EXTENSION\] The value a call of this function is compared equal
    /// to where the call stands for a predicate, as SQL/MM Full-Text's
    /// Contains, an INTEGER, is compared with 1 [ISO/IEC 13249-2:2002
    /// §5.1.2] (RULES.md D87). An index that finds rows for the function
    /// (`index_predicate`) finds them for `f(column, ?) = value` too, as
    /// for a call of a BOOLEAN result that is true. None by default.
    fn predicate_truth(&self) -> Option<Value> {
        None
    }
    /// \[EXTENSION\] Whether an index may find rows for this function
    /// though its calls can raise, as SQL/MM Full-Text's Contains raises
    /// of a malformed pattern (RULES.md D87). True promises what
    /// `index_predicate` does but for errors, and that a call raises only
    /// of its arguments other than the column, alike for every row; that a
    /// call of a null column is null, raising nothing; and that the index,
    /// given arguments that raise, finds every row it holds of a value. The
    /// WHERE check of the rows it finds then raises as a scan would, at the
    /// first row of a value. Defaults to false.
    fn rechecked_errors(&self) -> bool {
        false
    }
    /// \[EXTENSION\] How many of a call's first arguments an index finding
    /// rows for this function reads, the column and the value it is given;
    /// the rest only the WHERE check reads, as Contains's thesaurus
    /// relations, which the index answers as a superset (RULES.md D87).
    /// None, all of them, by default.
    fn probe_arguments(&self) -> Option<usize> {
        None
    }
    /// \[EXTENSION\] Of a call of an SQL/JSON query operator, or of
    /// JSON_CONTAINS, what an index reads of it: its operator, parsed path,
    /// PASSING names and behaviours (`JsonProbe`), so the planner finds
    /// rows of `JSON_EXISTS(doc, 'lax $.a')`, a call of one argument whose
    /// path the function holds (RULES.md D34). None by default.
    fn json_probe(&self) -> Option<JsonProbe<'_>> {
        None
    }
    /// \[EXTENSION\] A query whose ARRAY the binder gives a call as its
    /// last argument, read in the statement's snapshot as any subquery is,
    /// and with the privileges of its user: of SQL/MM Full-Text's Contains
    /// and Rank, the relations of the thesauri its pattern expands by
    /// (RULES.md D87). `constants` are the call's arguments that are
    /// constants, None of the others. None, nothing appended, by default;
    /// and nothing is if a table the query reads is not there.
    fn appended_query(&self, _constants: &[Option<&Value>]) -> Option<AppendedQuery> {
        None
    }
    /// \[EXTENSION\] As `appended_query`, given too the types of the
    /// call's arguments and the catalog it is bound in, so a query may
    /// read the tables the catalog registers, as SQL/MM Spatial's linear
    /// referencing reads the linear elements of the columns ST_LR_COLUMNS
    /// lists (`spatial/lr/sql/catalog.rs`). By default `appended_query`.
    fn catalog_query(
        &self,
        constants: &[Option<&Value>],
        _types: &[DataType],
        _catalog: &dyn crate::catalog::Catalog,
    ) -> Option<AppendedQuery> {
        self.appended_query(constants)
    }
    /// \[EXTENSION\] Whether the binder gives a call, before any appended
    /// query (`appended_query`), the name of the index that ranks the column
    /// its first argument reads, if it reads a base table's column one does:
    /// as SQL/MM Full-Text's Rank scores by that index's statistics
    /// (RULES.md D87). False by default.
    fn ranked_by_index(&self) -> bool {
        false
    }
    /// \[EXTENSION\] Whether a call is a score, greater the better, as
    /// SQL/MM Full-Text's Rank, whose highest an index of its first
    /// argument's column may give first (`Index::nearest`, RULES.md D87).
    /// True promises that a call is the same for the same arguments within
    /// a statement, its statistics fixed there (`CallContext`), and raises
    /// only as `rechecked_errors` says. False by default.
    fn ranks(&self) -> bool {
        false
    }
    /// \[EXTENSION\] Whether a call of `arity` arguments reads what its
    /// statement holds beside them (`CallContext`), so it is evaluated a
    /// row at a time by `call_in`, never by a batch. False by default.
    fn uses_context(&self, _arity: usize) -> bool {
        false
    }
    /// A call of `args` in the statement `cx` gives (`uses_context`). By
    /// default `call_with`.
    ///
    /// # Errors
    ///
    /// As `call_with`.
    fn call_in(&self, args: &[Value], cx: &dyn CallContext) -> Result<Value> {
        self.call_with(args, cx.temp_lobs())
    }
    /// A spatial operation whose complete behavior this function implements,
    /// including result types, NULLs and SRID errors (D85). Deterministic,
    /// side-effect-free, and fixed for its lifetime. Unlike `index_predicate`,
    /// this does not promise error-free calls: pruning requires a separate
    /// proof and a runtime fallback for changed prepared arguments.
    fn spatial_operation(&self) -> Option<crate::geometry::SpatialOperation> {
        None
    }
    /// [ISO/IEC 9075-2:2023 §8.20] The operator of a `<period predicate>`
    /// of two periods each named by a `<period reference>`, or of one and a
    /// point in time, whose call is false of a null bound and raises
    /// nothing \[GR4\], so a join may judge it as the comparisons it is
    /// made of (`physical/range/periods.rs`, RULES.md D581). None, by
    /// default.
    fn period_operator(&self) -> Option<crate::ast::PeriodOperator> {
        None
    }
    /// \[DESIGN\] The operation on arrays it is, if a compiled program may
    /// run one in place (`arrays::ArrayOperation`, RULES.md D914). None, by default.
    fn array_operation(&self) -> Option<ArrayOperation> {
        None
    }
    /// \[EXTENSION\] Whether "`f(args) op value`" holds, of a call of two
    /// arguments, decided without the call's value where the function
    /// decides it faster, as a prepared geometry operand decides a point's
    /// distance within a constant by its cells (RULES.md D85): the truth
    /// the comparison would have [X3.135-1986 §5.11 GR2, GR3], none of a
    /// null value, or the error the call would raise. None, by default,
    /// and the call is made and its value compared.
    fn call_compared(&self, _args: &[&Value], _op: CompOp, _value: &Value) -> Option<Result<Option<bool>>> {
        None
    }
    /// \[EXTENSION\] "`f(point, other) op value`", or with the point second,
    /// of a point geometry of `srid` at `point` and the constant operand
    /// `other` this function holds prepared (RULES.md D85): as
    /// `call_compared` would decide the call of the point's value, decided
    /// from the point's coordinates, as an index's entry holds them,
    /// without making its value. None unless the function holds `other`
    /// prepared; none by default.
    fn point_compared(&self, _point: ([f64; 2], i32), _other: &Value, _op: CompOp, _value: &Value) -> Option<Result<Option<bool>>> {
        None
    }
    /// \[EXTENSION\] Of a vector distance (vector design document §6), the
    /// metric a call of these constant arguments (none for each other one)
    /// measures by, upper case, and the position of its first vector, the
    /// second after it: a deterministic call, smaller the more similar its
    /// vectors. None of a call ordered otherwise (INNER_PRODUCT), of one
    /// whose metric is no constant, and by default (R13.1, D94).
    fn vector_metric(&self, _constants: &[Option<&Value>]) -> Option<(String, usize)> {
        None
    }
    /// Same arguments always give the same result? If so, the optimizer may
    /// evaluate calls with constant arguments once, at planning time.
    fn deterministic(&self) -> bool {
        true
    }
    /// \[EXTENSION\] Whether an identifier `word`, written alone as
    /// argument `position` (from 0), is a key word the function takes
    /// there, given as a character string of it, not read as a column:
    /// `first` says whether the first argument was one. So
    /// `VECTOR_DISTANCE(a, b, COSINE)` names its metric (RULES.md D93).
    /// Qualified, `s.w`, the word is written so. Default: none.
    fn word_argument(&self, _position: usize, _word: &str, _first: bool) -> bool {
        false
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
    /// \[EXTENSION\] Whether `call_many` is cheaper than a call a row, as a model's (vector design R12.3).
    fn batches(&self) -> bool {
        false
    }
    /// The values of calls of each of `rows`, in order: by default `call` of each.
    ///
    /// # Errors
    ///
    /// As `call`'s.
    fn call_many(&self, rows: &[Vec<Value>]) -> Result<Vec<Value>> {
        rows.iter().map(|r| self.call(r)).collect()
    }
    /// \[DESIGN\] As `call_many`, ending as the statement is, so a plug-in's call is cancelled (RULES.md D44, D170).
    ///
    /// # Errors
    ///
    /// As `call_many`'s, or the statement's cancellation.
    fn call_many_in(&self, rows: &[Vec<Value>], _statement: &dyn crate::exec::Subqueries) -> Result<Vec<Value>> {
        self.call_many(rows)
    }
    /// \[DESIGN\] (RULES.md D363) The values of a call of each of `rows`
    /// rows, its arguments `args`, each a vector of `rows` values or a
    /// constant, if the function has a body over vectors: a vector of
    /// `rows` values, each the one `call` gives of that row's arguments,
    /// or an error if a row's call would raise one, so the rows are then
    /// called one by one, in order. None, and each row is called, by
    /// default. Never called of a function that `warns`.
    fn eval_batch(&self, _args: &[Arg<'_>], _rows: usize) -> Option<Result<Column>> {
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
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §17.6 GR5] The type a `<dynamic
    /// parameter specification>` takes as field `position`, from 0, of its
    /// argument, if its argument is a ROW, as the binder makes the argument
    /// of a JSON aggregate; as its argument itself, of `position` 0, if
    /// not. None where it takes none, where it is refused \[GR4\]. By
    /// default none.
    fn parameter_type(&self, _position: usize) -> Option<ParameterType> {
        None
    }
    /// \[DESIGN\] (RULES.md D356) Which built-in set function it is, if it
    /// is one a grouping keeps in columns of states, a slot a group, in
    /// place of an accumulator a group. Only the core's COUNT, SUM, AVG,
    /// MAX and MIN tell one, as only the core makes a `Columnar`. Default:
    /// none.
    fn columnar(&self) -> Option<Columnar> {
        None
    }
    /// \[DESIGN\] (RULES.md D779) Whether its result is the collection of
    /// its values, an element of each row, as COLLECT's is, so of one row
    /// at most it has one element at most and raises nothing its argument
    /// and filter do not. Default: no.
    fn gathers(&self) -> bool {
        false
    }
}

// Unit tests: what the set functions refuse and how they total (RULES.md
// R-TEST-2 robustness).
#[cfg(test)]
mod borrowed;
#[cfg(test)]
mod tests;
