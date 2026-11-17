//! Tests of JSON_QUERY and of serializing an SQL/JSON sequence to a
//! character string type.

use super::*;
use crate::error::{Code, SqlError};
use crate::json::query::IMPLICIT_RETURNING;
use crate::json::query::tests::{integer, on, text, typed};
use crate::value::DataType;

/// The implicit returned type, as a static the clauses may borrow.
static RETURNING: DataType = IMPLICIT_RETURNING;

/// The clauses of a JSON_QUERY with no clause written: each implicit.
fn implicit() -> QueryOptions<'static> {
    QueryOptions {
        returning: &RETURNING,
        wrapper: Wrapper::Without,
        quotes: Quotes::Keep,
        on_empty: QueryBehavior::Null,
        on_error: QueryBehavior::Null,
    }
}

/// JSON_QUERY of the JSON text `context` and `path` with the clauses
/// `options`.
fn query(context: &Value, path: &str, options: &QueryOptions<'_>) -> Result<Warned> {
    on(context, path, &[], |common| json_query(common, options))
}

/// The text JSON_QUERY of `context` and `path` gives with `options`, or
/// none if it gives the null value.
fn text_of(context: &str, path: &str, options: &QueryOptions<'_>) -> Option<String> {
    let got = query(&text(context), path, options).unwrap().0;
    if got.is_null() {
        return None;
    }
    let Value::Char(s) = got else { panic!("{got:?}") };
    Some(s)
}

/// The SQLSTATE of the exception JSON_QUERY of `context` and `path`
/// raises with `options`.
fn state(context: &str, path: &str, options: &QueryOptions<'_>) -> String {
    query(&text(context), path, options).unwrap_err().sqlstate().as_str().to_owned()
}

/// A context item, a path, its arguments and the SQLSTATE of its
/// exception.
type Case<'a> = (Value, &'a str, &'a [(&'a str, Value, bool)], &'a str);

const CONTEXT: &str = r#"{"a": [1, "x"], "o": {"k": [true]}, "s": "abc", "n": null}"#;

/// Verifies: [ISO/IEC 9075-2:2023 §6.35 SR1b, SR6, SR9, GR5b,
/// §9.49 GR5c] with no clause written, the one item the path gives
/// serialized as JSON text, a string in quotation marks, the SQL/JSON
/// null `null`.
#[test]
fn the_one_item_is_serialized() {
    let options = implicit();
    assert_eq!(text_of(CONTEXT, "lax $.o", &options).as_deref(), Some(r#"{"k":[true]}"#));
    assert_eq!(text_of(CONTEXT, "strict $.a", &options).as_deref(), Some(r#"[1,"x"]"#));
    assert_eq!(text_of(CONTEXT, "lax $.s", &options).as_deref(), Some(r#""abc""#));
    assert_eq!(text_of(CONTEXT, "lax $.n", &options).as_deref(), Some("null"));
    assert_eq!(text_of(CONTEXT, "lax $.a[0]", &options).as_deref(), Some("1"));
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.49 GR5b i] OMIT QUOTES: a
/// character string alone is cast, not serialized, a string cut so with
/// its warning; any other item is serialized. MC/DC of OMIT QUOTES, a
/// scalar and a character string in `serialized`: a string cast, and
/// serialized KEEP QUOTES (`the_one_item_is_serialized`); an array and the
/// SQL/JSON null, no scalar, serialized; a truth value, a scalar of no
/// string, serialized `true` where a cast would give TRUE.
#[test]
fn omit_quotes_casts_a_string_alone() {
    let options = QueryOptions { quotes: Quotes::Omit, ..implicit() };
    assert_eq!(text_of(CONTEXT, "lax $.s", &options).as_deref(), Some("abc"));
    assert_eq!(text_of(CONTEXT, "lax $.a[0]", &options).as_deref(), Some("1"));
    assert_eq!(text_of(CONTEXT, "strict $.a", &options).as_deref(), Some(r#"[1,"x"]"#));
    assert_eq!(text_of(CONTEXT, "lax $.n", &options).as_deref(), Some("null"));
    assert_eq!(text_of(CONTEXT, "lax $.o.k[0]", &options).as_deref(), Some("true"));
    let short = DataType::varying(2);
    let (cut, warning) = query(&text(CONTEXT), "lax $.s", &QueryOptions { returning: &short, ..options }).unwrap();
    assert!(matches!(cut, Value::Char(s) if s == "ab"));
    assert_eq!(warning.map(|w| w.as_str().to_owned()).as_deref(), Some("01004"));
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.49 GR4a, GR4b] WITH
/// CONDITIONAL ARRAY wraps all but one array or object, a scalar alone
/// too; WITH UNCONDITIONAL ARRAY wraps all; WITHOUT ARRAY none, so more
/// than one item is SQLSTATE 22034. MC/DC of `single_array_or_object`:
/// one array and one object are left as they are, one string and two
/// items wrapped.
#[test]
fn the_wrapper_behaviours() {
    let conditional = QueryOptions { wrapper: Wrapper::Conditional, ..implicit() };
    let unconditional = QueryOptions { wrapper: Wrapper::Unconditional, ..implicit() };
    let without = QueryOptions { on_error: QueryBehavior::Error, ..implicit() };
    assert_eq!(text_of(CONTEXT, "strict $.a", &conditional).as_deref(), Some(r#"[1,"x"]"#));
    assert_eq!(text_of(CONTEXT, "lax $.o", &conditional).as_deref(), Some(r#"{"k":[true]}"#));
    assert_eq!(text_of(CONTEXT, "lax $.s", &conditional).as_deref(), Some(r#"["abc"]"#));
    assert_eq!(text_of(CONTEXT, "lax $.a[*]", &conditional).as_deref(), Some(r#"[1,"x"]"#));
    assert_eq!(text_of(CONTEXT, "strict $.a", &unconditional).as_deref(), Some(r#"[[1,"x"]]"#));
    assert_eq!(text_of(CONTEXT, "lax $.a[*]", &unconditional).as_deref(), Some(r#"[1,"x"]"#));
    assert_eq!(state(CONTEXT, "lax $.a[*]", &without), "22034");
    let e = query(&text(CONTEXT), "lax $.a[*]", &without).unwrap_err();
    assert_eq!(e.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.49 GR4c iii"));
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.35 SR4, §9.49 GR4a i,
/// GR4c i, GR5a] no item, wrapped or not [NOTE 501]: ON EMPTY NULL, the
/// default, gives the null value, EMPTY ARRAY `[]`, EMPTY OBJECT `{}`,
/// ERROR SQLSTATE 22035, which ON ERROR replaces.
#[test]
fn no_item_is_as_on_empty_says() {
    for wrapper in [Wrapper::Without, Wrapper::Conditional, Wrapper::Unconditional] {
        let with = |on_empty, on_error| QueryOptions { wrapper, on_empty, on_error, ..implicit() };
        assert_eq!(text_of(CONTEXT, "lax $.z", &with(QueryBehavior::Null, QueryBehavior::Error)), None);
        assert_eq!(text_of(CONTEXT, "lax $.z", &with(QueryBehavior::EmptyArray, QueryBehavior::Error)).as_deref(), Some("[]"));
        assert_eq!(text_of(CONTEXT, "lax $.z", &with(QueryBehavior::EmptyObject, QueryBehavior::Error)).as_deref(), Some("{}"));
        let e = query(&text(CONTEXT), "lax $.z", &with(QueryBehavior::Error, QueryBehavior::Error)).unwrap_err();
        assert_eq!(e.sqlstate().as_str(), "22035");
        assert_eq!(e.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.49 GR4c i 1"));
        assert_eq!(text_of(CONTEXT, "lax $.z", &with(QueryBehavior::Error, QueryBehavior::Null)), None);
        assert_eq!(text_of(CONTEXT, "lax $.z", &with(QueryBehavior::Error, QueryBehavior::EmptyArray)).as_deref(), Some("[]"));
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.35 SR5, §9.49 GR6] an
/// exception, of the path, of a context item that is no JSON text, of an
/// item with no JSON text or of a text too long, is replaced as ON ERROR
/// says: NULL, the default, the null value; EMPTY ARRAY and EMPTY OBJECT
/// their text; ERROR raises it.
#[test]
fn an_exception_is_replaced_as_on_error_says() {
    let short = DataType::varying(3);
    let date = typed("2020-01-02", &DataType::Date);
    let cases: [Case<'_>; 4] = [
        (text("{}"), "strict $.a", &[], "2203A"),
        (text("{"), "lax $", &[], "22032"),
        (text("0"), "lax $d", &[("d", date, false)], "22032"),
        (text(r#"{"a": [1, 2]}"#), "lax $.a", &[], "22001"),
    ];
    for (context, path, passing, expected) in cases {
        let with = |on_error| {
            let options = QueryOptions { returning: &short, on_error, ..implicit() };
            on(&context, path, passing, |common| json_query(common, &options)).map(|(value, _)| value)
        };
        assert!(with(QueryBehavior::Null).unwrap().is_null(), "{path}");
        assert!(matches!(with(QueryBehavior::EmptyArray).unwrap(), Value::Char(s) if s == "[]"));
        assert!(matches!(with(QueryBehavior::EmptyObject).unwrap(), Value::Char(s) if s == "{}"));
        assert_eq!(with(QueryBehavior::Error).unwrap_err().sqlstate().as_str(), expected, "{path}");
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.49 GR6a iii 2, §9.43 GR2a
/// iii] the empty array or object ON ERROR gives is serialized to the
/// returned type, and an exception of that is raised: `{}` is longer than
/// CHARACTER VARYING(1); of CHARACTER, the text is padded.
#[test]
fn the_text_on_error_gives_is_serialized_to_the_type() {
    let one = DataType::varying(1);
    let options = QueryOptions { returning: &one, on_error: QueryBehavior::EmptyObject, ..implicit() };
    assert_eq!(state("{", "lax $", &options), "22001");
    let fixed = DataType::character(4);
    let padded = QueryOptions { returning: &fixed, on_error: QueryBehavior::EmptyArray, ..implicit() };
    assert_eq!(text_of("{", "lax $", &padded).as_deref(), Some("[]  "));
    assert_eq!(text_of("[1]", "lax $", &padded).as_deref(), Some("[1] "));
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.35 GR1, §10.13 SR2] of a
/// null context item, the null value; the returned type is a string type,
/// another refused with SQLCODE -103, of a null context item too, a
/// binary string type with SQLCODE -900.
#[test]
fn a_null_context_item_and_the_returned_type() {
    assert!(query(&Value::Null, "strict $.a", &QueryOptions { on_error: QueryBehavior::Error, ..implicit() }).unwrap().0.is_null());
    let number = integer();
    for (to, code) in [(&number, -103_i32), (&DataType::VarBinary { length: 9 }, -900_i32)] {
        let options = QueryOptions { returning: to, ..implicit() };
        assert_eq!(query(&Value::Null, "lax $", &options).unwrap_err().sqlcode(), code, "{to}");
        assert_eq!(serialize_sequence(Ok(Vec::new()), &options).unwrap_err().sqlcode(), code, "{to}");
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.49 GR3, GR6] STATUS IN an
/// exception condition is replaced as ON ERROR says; an error not one, as
/// a Syntax Rule's, is raised whatever it says.
#[test]
fn the_status_in_is_replaced_or_raised() {
    let options = QueryOptions { on_error: QueryBehavior::EmptyArray, ..implicit() };
    let exception = Err(SqlError::new(Code::Data, "[DESIGN] an exception"));
    assert!(matches!(serialize_sequence(exception, &options).unwrap().0, Value::Char(s) if s == "[]"));
    let rule = Err(SqlError::new(Code::SyntaxRule, "[DESIGN] a rule"));
    assert_eq!(serialize_sequence(rule, &options).unwrap_err().sqlcode(), -103_i32);
    let e = on(&text("1"), "lax $x", &[], |common| json_query(common, &options)).unwrap_err();
    assert_eq!(e.sqlcode(), -103_i32);
}
