//! Unit tests of the SQL/JSON query operators' binding [ISO/IEC
//! 9075-2:2023 §6.28, §6.35, §8.23, §10.14, §9.46 SR8]: the implicit
//! clauses, the returned types, and the Syntax Rules checked as a
//! statement is prepared (RULES.md R-TEST-1).

use super::*;
use crate::ast::{UserFunction, ValueSpecification};
use crate::db::Database;
use crate::value::{DOUBLE_PRECISION, Spelled};

/// Schema S: table T of a character string, a number and a binary
/// string; a session of S.
fn session() -> crate::db::Session {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (C VARCHAR(20), N INTEGER, B VARBINARY(4))").unwrap();
    db.session("S")
}

/// The SQLCODE and the rule `sql` is refused with as it is prepared.
fn refused(sql: &str) -> (i32, String) {
    let e = session().prepare(sql).map(|_| panic!("{sql} prepared")).unwrap_err();
    (e.sqlcode(), e.rule.unwrap_or_default())
}

/// JSON_VALUE's clauses, of no RETURNING, and of `on_empty` and
/// `on_error`.
fn value(on_empty: Option<JsonValueBehavior>, on_error: Option<JsonValueBehavior>) -> JsonClauses {
    JsonClauses::Value { returning: None, on_empty, on_error }
}

/// DEFAULT CURRENT_USER.
fn default() -> JsonValueBehavior {
    JsonValueBehavior::Default(Box::new(ValueExpression::Value(ValueSpecification::User(UserFunction::Current))))
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.28 SR4, SR5; §6.35 SR4 to SR6, SR9;
/// §8.23 SR1] the implicit clauses: NULL ON EMPTY and NULL ON ERROR of
/// JSON_VALUE and JSON_QUERY, WITHOUT ARRAY and KEEP QUOTES of JSON_QUERY,
/// FALSE ON ERROR of JSON_EXISTS; a DEFAULT's value expression taken, ON
/// EMPTY's first.
#[test]
fn the_implicit_clauses_6_28() {
    let clauses = value(None, None);
    let (bound, defaults) = operation(&clauses).unwrap();
    let JsonOperation::Value { returning, on_empty: ValueSlot::Null, on_error: ValueSlot::Null } = bound else { panic!("{bound:?}") };
    assert_eq!(returning, IMPLICIT_RETURNING);
    assert!(defaults.is_empty());
    let written = JsonClauses::Query { returning: None, wrapper: None, quotes: None, on_empty: None, on_error: None };
    let (query, _) = operation(&written).unwrap();
    let JsonOperation::Query { wrapper, quotes, on_empty, on_error, .. } = query else { panic!("{query:?}") };
    assert_eq!((wrapper, quotes, on_empty, on_error), (Wrapper::Without, Quotes::Keep, QueryBehavior::Null, QueryBehavior::Null));
    let both = value(Some(default()), Some(default()));
    let (_, both_defaults) = operation(&both).unwrap();
    assert_eq!(both_defaults.len(), 2);
    let one = value(Some(JsonValueBehavior::Error), Some(default()));
    let (_, one_default) = operation(&one).unwrap();
    assert_eq!(one_default.len(), 1);
    let (exists, _) = operation(&JsonClauses::Exists { on_error: None }).unwrap();
    assert!(matches!(exists, JsonOperation::Exists(ExistsBehavior::False)));
    assert_eq!(slot(None), ValueSlot::Null);
    assert_eq!(slot(Some(&JsonValueBehavior::Error)), ValueSlot::Error);
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.28 SR2] JSON_VALUE returns a
/// character string, a number, a truth value or a datetime; a binary
/// string, an interval and a row type are refused -103. MC/DC of
/// `returnable`: each of its four kinds alone, and none.
#[test]
fn json_value_returns_a_predefined_scalar_6_28() {
    assert!(returnable(&DataType::varying(3)));
    assert!(returnable(&DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::DoublePrecision }));
    assert!(returnable(&DataType::Date));
    assert!(returnable(&DataType::Boolean));
    assert!(!returnable(&DataType::VarBinary { length: 1 }));
    for to in ["VARBINARY(4)", "INTERVAL DAY", "ROW (A INTEGER)"] {
        let (code, rule) = refused(&format!("VALUES JSON_VALUE('1', 'lax $' RETURNING {to})"));
        assert_eq!(code, -103_i32, "{to}");
        assert!(rule.contains("§6.28 SR2"), "{to}: {rule}");
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.14 SR1; §10.12 SR3; §9.46 SR8;
/// §9.48 GR4c iii] the operands checked as a statement is prepared: a
/// context item of no string type, a PASSING argument of a type the path
/// cannot take, a `starts with` variable that is no character string, and
/// a DEFAULT not cast to the returned type, each refused -103.
#[test]
fn the_operands_are_checked_10_14() {
    assert_eq!(refused("SELECT JSON_VALUE(N, 'lax $') FROM T").0, -103_i32);
    assert_eq!(refused("SELECT JSON_EXISTS(C, 'lax $ ? (@ starts with $X)' PASSING N AS X) FROM T").0, -103_i32);
    assert_eq!(refused("SELECT JSON_EXISTS(C, 'lax $ ? (@ starts with $X)' PASSING C FORMAT JSON AS X) FROM T").0, -103_i32);
    assert_eq!(refused("SELECT JSON_VALUE(C, 'lax $' RETURNING INTEGER DEFAULT DATE '2024-01-01' ON EMPTY) FROM T").0, -103_i32);
    assert_eq!(refused("SELECT JSON_QUERY(C, 'lax $' RETURNING JSON OMIT QUOTES) FROM T").0, -103_i32);
    assert!(session().prepare("SELECT JSON_EXISTS(C, 'lax $ ? (@ starts with $X)' PASSING C AS X) FROM T").is_ok());
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.48 GR4c iii, GR5d] a DEFAULT that
/// can raise is bound as the lazy CASE, one that cannot as an argument;
/// JSON_QUERY is never lazy. MC/DC of `lazy_value`: DEFAULT absent,
/// present and safe, present and raising.
#[test]
fn a_raising_default_is_bound_lazily_9_48() {
    let s = session();
    for (sql, lazy) in [
        ("SELECT JSON_VALUE(C, 'lax $' RETURNING INTEGER) FROM T", false),
        ("SELECT JSON_VALUE(C, 'lax $' RETURNING INTEGER DEFAULT 1 ON ERROR) FROM T", false),
        ("SELECT JSON_VALUE(C, 'lax $' RETURNING INTEGER DEFAULT 1 / N ON ERROR) FROM T", true),
    ] {
        let plan = s.explain(sql, &crate::params::Params::new()).unwrap();
        assert_eq!(plan.contains("TAKEN"), lazy, "{sql}: {plan}");
    }
}
