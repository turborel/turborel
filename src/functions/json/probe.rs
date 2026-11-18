//! \[EXTENSION\] What an index reads of a call of an SQL/JSON query
//! operator, or of JSON_CONTAINS (`ScalarFunction::json_probe`, RULES.md
//! D34): the operator, its parsed path, the names of its PASSING
//! arguments, its returned type and what its error and empty behaviours
//! do. A call holds its path inside the function, so `JSON_EXISTS(doc,
//! 'lax $.a')` is a call of one argument; this lets the planner and an
//! index read that path without knowing the function's own type.

use super::{JsonCall, JsonOperation, ValueSlot};
use crate::json::{Accessor, ExistsBehavior, Expr, Method, Path, Predicate, QueryBehavior, Subscript};
use crate::value::DataType;

impl JsonCall {
    /// \[EXTENSION\] The call as an index reads it.
    pub fn probe(&self) -> JsonProbe<'_> {
        let (operator, returning, on_error, empty_null) = match &self.operation {
            JsonOperation::Exists(behavior) => {
                let on_error = match behavior {
                    ExistsBehavior::Error => OnError::Raised,
                    ExistsBehavior::False | ExistsBehavior::Unknown => OnError::Null,
                    ExistsBehavior::True => OnError::Value,
                };
                (JsonOperator::Exists, None, on_error, true)
            }
            JsonOperation::Value { returning, on_empty, on_error } => {
                (JsonOperator::Value, Some(returning), slot_error(*on_error), *on_empty == ValueSlot::Null)
            }
            JsonOperation::Query { returning, on_empty, on_error, .. } => {
                (JsonOperator::Query, Some(returning), query_error(*on_error), *on_empty == QueryBehavior::Null)
            }
        };
        JsonProbe { operator, path: Some(&self.path), passing: &self.passing, returning, on_error, empty_null }
    }

    /// \[EXTENSION\] Whether the call keys an index safely
    /// (`JsonProbe::index_safe`): an error row then indexes as null.
    pub fn index_safe(&self) -> bool {
        self.probe().index_safe()
    }
}

/// What a JSON_VALUE ON ERROR does.
fn slot_error(slot: ValueSlot) -> OnError {
    match slot {
        ValueSlot::Error => OnError::Raised,
        ValueSlot::Null => OnError::Null,
        ValueSlot::Default => OnError::Value,
    }
}

/// What a JSON_QUERY ON ERROR does.
fn query_error(behavior: QueryBehavior) -> OnError {
    match behavior {
        QueryBehavior::Error => OnError::Raised,
        QueryBehavior::Null => OnError::Null,
        QueryBehavior::EmptyArray | QueryBehavior::EmptyObject => OnError::Value,
    }
}

/// The operator a call is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonOperator {
    /// [ISO/IEC 9075-2:2023 §8.23] JSON_EXISTS.
    Exists,
    /// [ISO/IEC 9075-2:2023 §6.28] JSON_VALUE.
    Value,
    /// [ISO/IEC 9075-2:2023 §6.35] JSON_QUERY.
    Query,
    /// \[EXTENSION\] JSON_CONTAINS, of a document and a candidate.
    Contains,
}

/// What a call's ON ERROR does with an exception condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnError {
    /// It is raised.
    Raised,
    /// It gives the null value, or, of JSON_EXISTS, False or Unknown:
    /// never a value a lookup could miss.
    Null,
    /// It gives a value of its own: True, a DEFAULT, an empty array or
    /// object.
    Value,
}

/// \[EXTENSION\] A call of an SQL/JSON operator as an index reads it,
/// borrowed of the function (`ScalarFunction::json_probe`).
#[derive(Debug, Clone, Copy)]
pub struct JsonProbe<'a> {
    /// The operator.
    pub operator: JsonOperator,
    /// The parsed path; none of JSON_CONTAINS.
    pub path: Option<&'a Path>,
    /// Each PASSING argument's name and whether it is FORMAT JSON, in
    /// the order of the call's arguments after the context item.
    pub passing: &'a [(String, bool)],
    /// The returned type; none of JSON_EXISTS and JSON_CONTAINS.
    pub returning: Option<&'a DataType>,
    /// What ON ERROR does.
    pub on_error: OnError,
    /// Whether an empty result is the null value (NULL ON EMPTY); always
    /// of JSON_EXISTS and JSON_CONTAINS.
    pub empty_null: bool,
}

impl JsonProbe<'_> {
    /// \[EXTENSION\] Whether a call is safe to key an index by (RULES.md
    /// D15): no exception condition is raised, as ON ERROR is NULL, its
    /// path reads no datetime of the session's time zone (`datetime()`,
    /// `time_tz()`, `timestamp_tz()`), it returns no type with a time
    /// zone, and every named variable has an argument. A DEFAULT ON ERROR
    /// is safe only if its value casts, which its caller checks.
    pub fn index_safe(&self) -> bool {
        let caught = self.on_error != OnError::Raised;
        let names: Vec<&str> = self.passing.iter().map(|(name, _)| name.as_str()).collect();
        let bound = self.path.is_none_or(|p| p.check_variables(&names).is_ok());
        caught && bound && self.path.is_none_or(|p| !zoned_expr(&p.expr, 0)) && self.returning.is_none_or(|t| !zoned_type(t))
    }

    /// Whether a lookup of the call finds every row it is true of, and
    /// raises only where a scan would: its exception conditions give no
    /// value but null, False or Unknown (`OnError::Null`), and every named
    /// variable has an argument. Rows the index cannot read are found by
    /// every probe (RULES.md D34).
    pub fn lookup_safe(&self) -> bool {
        let names: Vec<&str> = self.passing.iter().map(|(name, _)| name.as_str()).collect();
        self.on_error == OnError::Null && self.path.is_none_or(|p| p.check_variables(&names).is_ok())
    }
}

/// [ISO/IEC 9075-2:2023 §6.1] Whether `t` is a datetime type WITH TIME ZONE.
fn zoned_type(t: &DataType) -> bool {
    matches!(t, DataType::Time { zoned: true, .. } | DataType::Timestamp { zoned: true, .. })
}

/// \[DESIGN\] How deep the walk of a path goes: a parsed path nests no
/// deeper than its own bound (`json/path.rs`), so this never cuts one.
const MOST_DEPTH: usize = 256;

/// Whether `expr`, `depth` nodes down, reads a datetime by the session's
/// time zone, or is deeper than a walk goes, which counts as reading one.
fn zoned_expr(expr: &Expr, depth: usize) -> bool {
    if depth > MOST_DEPTH {
        return true;
    }
    let next = depth.saturating_add(1);
    match expr {
        Expr::Literal(_) | Expr::Context | Expr::Variable(_) | Expr::Current | Expr::Last => false,
        Expr::Access(base, accessor) => zoned_expr(base, next) || zoned_accessor(accessor, next),
        Expr::Signed(_, operand) => zoned_expr(operand, next),
        Expr::Arithmetic(left, _, right) => zoned_expr(left, next) || zoned_expr(right, next),
    }
}

/// Whether `accessor` reads a datetime by the session's time zone.
fn zoned_accessor(accessor: &Accessor, depth: usize) -> bool {
    match accessor {
        Accessor::Member(_) | Accessor::AnyMember | Accessor::AnyElement => false,
        Accessor::Subscripts(subscripts) => {
            subscripts.iter().any(|s: &Subscript| zoned_expr(&s.from, depth) || s.to.as_ref().is_some_and(|to| zoned_expr(to, depth)))
        }
        Accessor::Filter(predicate) => zoned_predicate(predicate, depth),
        Accessor::Method(method) => zoned_method(method),
    }
}

/// Whether `method` gives a datetime by the session's time zone.
fn zoned_method(method: &Method) -> bool {
    match method {
        Method::Datetime | Method::DatetimeTemplate(_) | Method::TimeTz(_) | Method::TimestampTz(_) => true,
        Method::Type
        | Method::Size
        | Method::Double
        | Method::Ceiling
        | Method::Floor
        | Method::Abs
        | Method::Keyvalue
        | Method::Bigint
        | Method::Boolean
        | Method::Date
        | Method::Decimal { .. }
        | Method::Integer
        | Method::Number
        | Method::String
        | Method::Time(_)
        | Method::Timestamp(_) => false,
    }
}

/// Whether `predicate` reads a datetime by the session's time zone.
fn zoned_predicate(predicate: &Predicate, depth: usize) -> bool {
    let next = depth.saturating_add(1);
    if depth > MOST_DEPTH {
        return true;
    }
    match predicate {
        Predicate::Exists(e) | Predicate::LikeRegex(e, _) => zoned_expr(e, next),
        Predicate::Compare(a, _, b) | Predicate::StartsWith(a, b) => zoned_expr(a, next) || zoned_expr(b, next),
        Predicate::IsUnknown(p) | Predicate::Not(p) => zoned_predicate(p, next),
        Predicate::And(a, b) | Predicate::Or(a, b) => zoned_predicate(a, next) || zoned_predicate(b, next),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse_path;
    use crate::value::datetime::Clocked;

    fn parse_path_now(text: &str) -> crate::error::Result<Path> {
        parse_path(text, Clocked::default())
    }

    fn probe(path: &Path, on_error: OnError, returning: Option<&DataType>) -> bool {
        let p = JsonProbe { operator: JsonOperator::Value, path: Some(path), passing: &[], returning, on_error, empty_null: true };
        p.index_safe()
    }

    /// Verifies: [EXTENSION] a JSON_VALUE call keys an index only with
    /// NULL ON ERROR, a path reading no zoned datetime and a returned type
    /// without a time zone. MC/DC: each condition alone makes it unsafe.
    #[test]
    fn a_call_is_index_safe_only_without_errors_or_zones() {
        let plain = parse_path_now("lax $.a").unwrap();
        let zoned = parse_path_now("lax $.a.timestamp_tz()").unwrap();
        let filtered = parse_path_now("lax $.a ? (@.datetime() == $x)").unwrap();
        assert!(probe(&plain, OnError::Null, None));
        assert!(!probe(&plain, OnError::Raised, None), "ERROR ON ERROR");
        assert!(!probe(&zoned, OnError::Null, None), "timestamp_tz()");
        assert!(!probe(&filtered, OnError::Null, None), "datetime() in a filter");
        let tz = DataType::Timestamp { precision: 6, zoned: true };
        assert!(!probe(&plain, OnError::Null, Some(&tz)), "WITH TIME ZONE");
        let unzoned = DataType::Timestamp { precision: 6, zoned: false };
        assert!(probe(&plain, OnError::Null, Some(&unzoned)));
    }

    /// Verifies: [EXTENSION] a lookup is safe only if ON ERROR gives no
    /// value a lookup could miss and every variable is passed.
    #[test]
    fn a_lookup_is_safe_only_of_caught_errors_and_named_variables() {
        let path = parse_path_now("lax $.a ? (@ == $v)").unwrap();
        let passing = [("v".to_owned(), false)];
        let p = JsonProbe {
            operator: JsonOperator::Exists,
            path: Some(&path),
            passing: &passing,
            returning: None,
            on_error: OnError::Null,
            empty_null: true,
        };
        assert!(p.lookup_safe());
        assert!(!JsonProbe { on_error: OnError::Value, ..p }.lookup_safe(), "TRUE ON ERROR");
        assert!(!JsonProbe { passing: &[], ..p }.lookup_safe(), "a variable not passed");
    }
}
