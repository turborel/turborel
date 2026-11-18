//! Unit tests of the Formats of the SQL/JSON query operators [ISO/IEC
//! 9075-2:2023 §6.28, §6.35, §8.23, §10.14] and of the `<JSON predicate>`
//! and constructors [§6.34, §8.22]: the clauses read, the implicit ones
//! left unwritten, and the text refused.

use super::*;
use crate::ast::JsonConstructor;

/// `text`, parsed as a value expression.
fn parsed(text: &str) -> Result<ValueExpression> {
    Parser::new(text)?.value_expression()
}

/// The operator `text` parses to, a JSON_VALUE or JSON_QUERY.
fn operator(text: &str) -> JsonOperator {
    match parsed(text) {
        Ok(ValueExpression::Json(json, _)) => match *json {
            JsonExpression::Query(operator) => *operator,
            other @ (JsonExpression::Constructor(_) | JsonExpression::Typed(_)) => panic!("{text}: {other:?}"),
        },
        other => panic!("{text}: {other:?}"),
    }
}

/// The SQLCODE of the error `text` is refused with.
fn refused(text: &str) -> i32 {
    parsed(text).map(|e| panic!("{text}: {e:?}")).unwrap_err().sqlcode()
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.28; §10.14] JSON_VALUE reads its
/// common syntax, PASSING's arguments in order, RETURNING, and each pair
/// of behaviors: ON EMPTY alone, ON ERROR alone, both, a DEFAULT of any
/// value expression; none written is none.
#[test]
fn json_value_reads_each_clause_6_28() {
    let read = operator("JSON_VALUE(J, 'lax $.a ? (@ > $x)' PASSING 1 AS X, J FORMAT JSON AS \"y\" RETURNING INTEGER NULL ON EMPTY)");
    assert_eq!(read.common.path, "lax $.a ? (@ > $x)");
    let names: Vec<&str> = read.common.passing.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["X", "y"]);
    assert!(read.common.passing.get(1).is_some_and(|a| a.input.format.is_some()));
    let JsonClauses::Value { returning: Some(_), on_empty: Some(JsonValueBehavior::Null), on_error: None } = read.clauses else {
        panic!("{:?}", read.clauses)
    };
    let on_error = operator("JSON_VALUE(J, 'lax $' DEFAULT 1 + 2 ON ERROR)");
    assert!(matches!(on_error.clauses, JsonClauses::Value { on_empty: None, on_error: Some(JsonValueBehavior::Default(_)), .. }));
    let both = operator("JSON_VALUE(J, 'lax $' ERROR ON EMPTY DEFAULT 'x' ON ERROR)");
    assert!(matches!(
        both.clauses,
        JsonClauses::Value { on_empty: Some(JsonValueBehavior::Error), on_error: Some(JsonValueBehavior::Default(_)), .. }
    ));
    assert!(matches!(operator("JSON_VALUE(J, 'lax $')").clauses, JsonClauses::Value { returning: None, on_empty: None, on_error: None }));
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.28; §10.14 SR2a] what JSON_VALUE
/// refuses: behaviors in the wrong order, a behavior of no ON, a path that
/// is no literal, and AS a path name, which only JSON_TABLE has.
#[test]
fn json_value_refuses_6_28() {
    for text in [
        "JSON_VALUE(J, 'lax $' NULL ON ERROR NULL ON EMPTY)",
        "JSON_VALUE(J, 'lax $' NULL)",
        "JSON_VALUE(J, P)",
        "JSON_VALUE(J, 'lax $' NULL ON NOTHING)",
    ] {
        assert_eq!(refused(text), -102_i32, "{text}");
    }
    assert_eq!(refused("JSON_VALUE(J, 'lax $' AS P)"), -103_i32);
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.35 SR7, SR8] JSON_QUERY's wrapper:
/// WITH alone is UNCONDITIONAL, ARRAY optional; a quotes behavior with
/// WITHOUT is read, with WITH CONDITIONAL or WITH UNCONDITIONAL refused
/// -103 (MC/DC of the refusal: quotes written, and a wrapper that wraps);
/// EMPTY ARRAY and EMPTY OBJECT read.
#[test]
fn json_query_wrapper_and_quotes_6_35() {
    let wrapper = |text: &str| match operator(text).clauses {
        JsonClauses::Query { wrapper, quotes, .. } => (wrapper, quotes),
        other @ (JsonClauses::Exists { .. } | JsonClauses::Value { .. }) => panic!("{other:?}"),
    };
    assert_eq!(wrapper("JSON_QUERY(J, 'lax $' WITH WRAPPER)"), (Some(Wrapper::Unconditional), None));
    assert_eq!(wrapper("JSON_QUERY(J, 'lax $' WITH CONDITIONAL ARRAY WRAPPER)"), (Some(Wrapper::Conditional), None));
    assert_eq!(wrapper("JSON_QUERY(J, 'lax $' WITHOUT WRAPPER OMIT QUOTES)"), (Some(Wrapper::Without), Some(Quotes::Omit)));
    assert_eq!(wrapper("JSON_QUERY(J, 'lax $' KEEP QUOTES ON SCALAR STRING)"), (None, Some(Quotes::Keep)));
    assert_eq!(refused("JSON_QUERY(J, 'lax $' WITH WRAPPER OMIT QUOTES)"), -103_i32);
    assert_eq!(refused("JSON_QUERY(J, 'lax $' WITH CONDITIONAL WRAPPER KEEP QUOTES)"), -103_i32);
    let JsonClauses::Query { on_empty, on_error, .. } =
        operator("JSON_QUERY(J, 'lax $' EMPTY ARRAY ON EMPTY EMPTY OBJECT ON ERROR)").clauses
    else {
        panic!("no JSON_QUERY")
    };
    assert_eq!((on_empty, on_error), (Some(QueryBehavior::EmptyArray), Some(QueryBehavior::EmptyObject)));
    assert_eq!(refused("JSON_QUERY(J, 'lax $' EMPTY ON EMPTY)"), -102_i32);
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.23] JSON_EXISTS reads each `<JSON
/// exists error behavior>` before ON ERROR, and none.
#[test]
fn json_exists_reads_its_behavior_8_23() {
    for (text, behavior) in [
        ("JSON_EXISTS(J, 'lax $' TRUE ON ERROR)", Some(ExistsBehavior::True)),
        ("JSON_EXISTS(J, 'lax $' FALSE ON ERROR)", Some(ExistsBehavior::False)),
        ("JSON_EXISTS(J, 'lax $' UNKNOWN ON ERROR)", Some(ExistsBehavior::Unknown)),
        ("JSON_EXISTS(J, 'lax $' ERROR ON ERROR)", Some(ExistsBehavior::Error)),
        ("JSON_EXISTS(J, 'lax $')", None),
    ] {
        let Ok(Predicate::JsonExists(read)) = Parser::new(text).and_then(|mut p| p.json_exists()) else { panic!("{text}") };
        let JsonClauses::Exists { on_error } = read.clauses else { panic!("{text}") };
        assert_eq!(on_error, behavior, "{text}");
    }
    assert!(Parser::new("JSON_EXISTS(J, 'lax $' TRUE ON EMPTY)").and_then(|mut p| p.json_exists()).is_err());
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.34 SR1; §8.22] the constructors read
/// as constructors, JSON_ARRAY of a query too, and FORMAT JSON
/// ENCODING before IS JSON read as the predicate's input.
#[test]
fn constructors_and_the_predicate_6_34() {
    let constructor = |text: &str| match parsed(text) {
        Ok(ValueExpression::Json(json, _)) => match *json {
            JsonExpression::Constructor(c) => c,
            other @ (JsonExpression::Query(_) | JsonExpression::Typed(_)) => panic!("{text}: {other:?}"),
        },
        other => panic!("{text}: {other:?}"),
    };
    assert!(matches!(constructor("JSON_OBJECT('a' VALUE 1, 'b' : 2 ABSENT ON NULL)"), JsonConstructor::Object { .. }));
    assert!(matches!(constructor("JSON_ARRAY(1, 2 NULL ON NULL)"), JsonConstructor::Array { .. }));
    assert!(matches!(constructor("JSON_ARRAY(SELECT A FROM T)"), JsonConstructor::ArrayQuery { .. }));
    for text in ["J IS JSON", "J IS NOT JSON ARRAY", "J FORMAT JSON IS JSON", "J FORMAT JSON ENCODING UTF8 IS JSON OBJECT"] {
        assert!(parsed(text).is_ok(), "{text}");
    }
    assert!(parsed("J FORMAT JSON ENCODING UTF8 IS").is_err());
}
