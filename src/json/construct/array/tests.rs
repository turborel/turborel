//! Tests of JSON_ARRAY, by enumeration and by query.

use super::*;
use crate::json::construct::tests::{formatted, json, plain, text};
use crate::value::Decimal;
use crate::value::lob::LobKind;
use crate::value::{CharUnits, cast};

/// The JSON text of JSON_ARRAY of `elements` and `options`.
fn array_text(elements: &[Input<'_>], options: &ArrayOptions<'_>) -> String {
    json(&json_array(elements, options).unwrap())
}

/// The options of NULL ON NULL, RETURNING the implicit type.
const NULL_ON_NULL: ArrayOptions<'static> = ArrayOptions { on_null: OnNull::Null, ..ArrayOptions::IMPLICIT };

/// Verifies: [ISO/IEC 9075-2:2023 §6.34 SR7a, SR2b] the
/// implicit clauses: ABSENT ON NULL, RETURNING CHARACTER VARYING of the
/// most characters (ID098).
#[test]
fn the_implicit_clauses() {
    assert_eq!(ArrayOptions::IMPLICIT.on_null, OnNull::Absent);
    assert_eq!(*ArrayOptions::IMPLICIT.returning, IMPLICIT_RETURNING);
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.34 GR3c i, GR3c ii, GR5a]
/// the JSON text of an array of the elements given, in order: none of
/// none; a number, a string, a truth value, a datetime cast to a string,
/// a FORMAT JSON value parsed.
#[test]
fn an_array_of_the_elements_given() {
    assert_eq!(array_text(&[], &ArrayOptions::IMPLICIT), "[]");
    let number = Value::Exact(Decimal::new(7_i128, 0_u32));
    let string = text("say \"hi\"");
    let date = cast(text("2020-01-02"), &DataType::Date).unwrap();
    let parsed = text(r#"{"a": 1, "a": 2}"#);
    let elements = [plain(&number), plain(&string), plain(&Value::Bool(true)), plain(&date), formatted(&parsed)];
    assert_eq!(array_text(&elements, &ArrayOptions::IMPLICIT), r#"[7,"say \"hi\"",true,"2020-01-02",{"a":1,"a":2}]"#);
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.34 SR7a, GR3c ii 2]
/// ABSENT ON NULL, the default, leaves out the SQL/JSON nulls, of the
/// null value and of FORMAT JSON null; NULL ON NULL keeps them. MC/DC of
/// ABSENT ON NULL and the SQL/JSON null in `add`: null left out ABSENT ON
/// NULL, kept NULL ON NULL; `"1"` kept ABSENT ON NULL.
#[test]
fn the_null_clause() {
    let one = text("1");
    let null = text("null");
    let elements = [plain(&Value::Null), plain(&one), formatted(&null), formatted(&Value::Null)];
    assert_eq!(array_text(&elements, &ArrayOptions::IMPLICIT), r#"["1"]"#);
    assert_eq!(array_text(&elements, &NULL_ON_NULL), r#"[null,"1",null,null]"#);
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.34 GR3c ii 1 B III] a
/// FORMAT JSON element that is no JSON text is SQLSTATE 22032; one not a
/// string SQLCODE -103.
#[test]
fn a_format_json_element_is_json_text() {
    let broken = text("[1,]");
    assert_eq!(json_array(&[formatted(&broken)], &ArrayOptions::IMPLICIT).unwrap_err().sqlstate().as_str(), "22032");
    let truth = Value::Bool(true);
    assert_eq!(json_array(&[formatted(&truth)], &ArrayOptions::IMPLICIT).unwrap_err().sqlcode(), -103_i32);
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.34 SR4, GR5a, §9.43 GR2a
/// iii] the text is a value of the type returned: one longer than it
/// SQLSTATE 22001; a type no string's SQLCODE -103, before any element
/// is read; a binary string type's -900.
#[test]
fn the_type_returned() {
    let one = Value::Bool(true);
    let fits = ArrayOptions { returning: &DataType::varying(6), ..ArrayOptions::IMPLICIT };
    assert_eq!(array_text(&[plain(&one)], &fits), "[true]");
    let short = ArrayOptions { returning: &DataType::varying(5), ..ArrayOptions::IMPLICIT };
    assert_eq!(json_array(&[plain(&one)], &short).unwrap_err().sqlstate().as_str(), "22001");
    let broken = text("[");
    let boolean = ArrayOptions { returning: &DataType::Boolean, ..ArrayOptions::IMPLICIT };
    assert_eq!(json_array(&[formatted(&broken)], &boolean).unwrap_err().sqlcode(), -103_i32);
    let binary = DataType::Lob { kind: LobKind::Binary, length: 100_u64, units: CharUnits::Octets };
    let blob = ArrayOptions { returning: &binary, ..ArrayOptions::IMPLICIT };
    assert_eq!(json_array(&[plain(&one)], &blob).unwrap_err().sqlcode(), -900_i32);
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.34 GR4b] by query, the
/// array of the column's values in the rows' order, none of no rows; each
/// value parsed if FORMAT JSON is written, else not; the null clause as
/// by enumeration.
#[test]
fn an_array_by_query() {
    assert_eq!(json(&json_array_by_query(&[], false, &ArrayOptions::IMPLICIT).unwrap()), "[]");
    let column = [text("[1]"), Value::Null, text("{}")];
    assert_eq!(json(&json_array_by_query(&column, false, &ArrayOptions::IMPLICIT).unwrap()), r#"["[1]","{}"]"#);
    assert_eq!(json(&json_array_by_query(&column, true, &ArrayOptions::IMPLICIT).unwrap()), "[[1],{}]");
    assert_eq!(json(&json_array_by_query(&column, true, &NULL_ON_NULL).unwrap()), "[[1],null,{}]");
    let broken = [text("{")];
    assert_eq!(json_array_by_query(&broken, true, &ArrayOptions::IMPLICIT).unwrap_err().sqlstate().as_str(), "22032");
}
