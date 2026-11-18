//! Tests of JSON_OBJECTAGG, and of JSON_ARRAYAGG as JSON_ARRAY by query
//! of its rows.

use super::*;
use crate::json::construct::tests::{formatted, json, plain, text};
use crate::json::construct::{ArrayOptions, OnNull};
use crate::json::{IMPLICIT_RETURNING, Keys};

/// JSON_OBJECTAGG of the rows `rows`, each a name and an input, and
/// `options`.
fn object_agg(rows: &[(&Value, Input<'_>)], options: &ObjectOptions<'_>) -> Result<Value> {
    let mut state = JsonObjectAgg::new(options)?;
    for (name, value) in rows {
        state.add(name, value)?;
    }
    state.finish()
}

/// JSON_ARRAYAGG of the rows `rows`, of one FORMAT JSON or not, and
/// `options`.
fn array_agg(rows: &[Input<'_>], options: &ArrayOptions<'_>) -> Result<Value> {
    super::super::json_array(rows, options)
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.11 GR4b i] of no rows
/// JSON_OBJECTAGG is the null value, not an empty object.
#[test]
fn no_rows_is_the_null_value() {
    assert!(object_agg(&[], &ObjectOptions::IMPLICIT).unwrap().is_null());
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.11 SR4a, SR4b, GR4b ii,
/// GR6b] JSON_OBJECTAGG is the object of its rows' members, in order: NULL
/// ON NULL, the default, keeps one bound to the SQL/JSON null, ABSENT ON
/// NULL leaves it out, an empty object if all are; WITHOUT UNIQUE KEYS,
/// the default, keeps two of one key.
#[test]
fn an_object_of_the_rows() {
    let (a, b) = (text("a"), text("b"));
    let one = Value::Bool(true);
    let parsed = text("[1]");
    let rows = [(&a, plain(&one)), (&b, plain(&Value::Null)), (&a, formatted(&parsed))];
    assert_eq!(json(&object_agg(&rows, &ObjectOptions::IMPLICIT).unwrap()), r#"{"a":true,"b":null,"a":[1]}"#);
    let absent = ObjectOptions { on_null: OnNull::Absent, ..ObjectOptions::IMPLICIT };
    assert_eq!(json(&object_agg(&rows, &absent).unwrap()), r#"{"a":true,"a":[1]}"#);
    assert_eq!(json(&object_agg(&[(&b, plain(&Value::Null))], &absent).unwrap()), "{}");
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.11 GR4b ii 2 B, GR4b ii 3]
/// a row whose name is the null value is SQLSTATE 22004, raised as the
/// row is given; WITH UNIQUE KEYS two rows of one name are 22030, raised
/// by `finish`, after every row's own errors; a FORMAT JSON value is
/// parsed under it, 22037.
#[test]
fn errors_of_the_rows() {
    let a = text("a");
    let one = Value::Bool(true);
    let mut state = JsonObjectAgg::new(&ObjectOptions::IMPLICIT).unwrap();
    assert_eq!(state.add(&Value::Null, &plain(&one)).unwrap_err().sqlstate().as_str(), "22004");
    let unique = ObjectOptions { keys: Keys::Unique, ..ObjectOptions::IMPLICIT };
    let mut twice = JsonObjectAgg::new(&unique).unwrap();
    twice.add(&a, &plain(&one)).unwrap();
    twice.add(&a, &plain(&one)).unwrap();
    assert_eq!(twice.add(&Value::Null, &plain(&one)).unwrap_err().sqlstate().as_str(), "22004");
    let e = twice.finish().unwrap_err();
    assert_eq!(e.sqlstate().as_str(), "22030");
    assert_eq!(e.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §6.34 GR2d ii 2"));
    let inner = text(r#"{"k": 1, "k": 2}"#);
    assert_eq!(object_agg(&[(&a, formatted(&inner))], &unique).unwrap_err().sqlstate().as_str(), "22037");
    assert_eq!(json(&object_agg(&[(&a, formatted(&inner))], &ObjectOptions::IMPLICIT).unwrap()), r#"{"a":{"k":1,"k":2}}"#);
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.11 SR5a, GR5f ii, GR5g,
/// GR6b] JSON_ARRAYAGG is the array of its rows' values in the order
/// given: ABSENT ON NULL, the default, leaves out the SQL/JSON nulls, an
/// empty array if all are; NULL ON NULL keeps them; FORMAT JSON values
/// are parsed, 22032 of one that is no JSON text.
#[test]
fn an_array_of_the_rows() {
    let (x, y) = (text("x"), text(r#"{"a": 2}"#));
    let rows = [plain(&x), plain(&Value::Null), formatted(&y), formatted(&Value::Null)];
    assert_eq!(json(&array_agg(&rows, &ArrayOptions::IMPLICIT).unwrap()), r#"["x",{"a":2}]"#);
    let null_on_null = ArrayOptions { on_null: OnNull::Null, ..ArrayOptions::IMPLICIT };
    assert_eq!(json(&array_agg(&rows, &null_on_null).unwrap()), r#"["x",null,{"a":2},null]"#);
    assert_eq!(json(&array_agg(&[plain(&Value::Null)], &ArrayOptions::IMPLICIT).unwrap()), "[]");
    let broken = text("[");
    assert_eq!(array_agg(&[formatted(&broken)], &ArrayOptions::IMPLICIT).unwrap_err().sqlstate().as_str(), "22032");
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.11 SR1b, SR3, GR6b, §9.43
/// GR2a iii] the type returned: the implicit one (ID134); a text longer
/// than it SQLSTATE 22001 by `finish`; a type no string's SQLCODE -103 and
/// a binary string type's -900 by `new`.
#[test]
fn the_type_returned() {
    assert_eq!(*ObjectOptions::IMPLICIT.returning, IMPLICIT_RETURNING);
    let one = Value::Bool(true);
    let short = ArrayOptions { returning: &DataType::varying(5), ..ArrayOptions::IMPLICIT };
    assert_eq!(array_agg(&[plain(&one)], &short).unwrap_err().sqlstate().as_str(), "22001");
    let a = text("a");
    let tiny = ObjectOptions { returning: &DataType::varying(9), ..ObjectOptions::IMPLICIT };
    assert_eq!(object_agg(&[(&a, plain(&one))], &tiny).unwrap_err().sqlstate().as_str(), "22001");
    let boolean = ArrayOptions { returning: &DataType::Boolean, ..ArrayOptions::IMPLICIT };
    assert_eq!(array_agg(&[], &boolean).unwrap_err().sqlcode(), -103_i32);
    let binary = DataType::VarBinary { length: 10_u32 };
    assert_eq!(JsonObjectAgg::new(&ObjectOptions { returning: &binary, ..ObjectOptions::IMPLICIT }).unwrap_err().sqlcode(), -900_i32);
    assert_eq!(
        JsonObjectAgg::new(&ObjectOptions { returning: &DataType::Date, ..ObjectOptions::IMPLICIT }).unwrap_err().sqlcode(),
        -103_i32
    );
}
