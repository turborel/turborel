//! Tests of JSON_OBJECT.

use super::*;
use crate::json::construct::tests::{formatted, json, plain, text};
use crate::value::Decimal;
use crate::value::lob::{Lob, LobKind};
use crate::value::{CharUnits, cast};

/// JSON_OBJECT of `pairs`, each a name and an input, and `options`.
fn object(pairs: &[(&Value, Input<'_>)], options: &ObjectOptions<'_>) -> Result<Value> {
    let members: Vec<NameAndValue<'_>> = pairs.iter().map(|(name, value)| NameAndValue { name, value: *value }).collect();
    json_object(&members, options)
}

/// The JSON text of JSON_OBJECT of `pairs` and `options`.
fn object_text(pairs: &[(&Value, Input<'_>)], options: &ObjectOptions<'_>) -> String {
    json(&object(pairs, options).unwrap())
}

/// The options of `on_null` and `keys`, RETURNING the implicit type.
fn of(on_null: OnNull, keys: Keys) -> ObjectOptions<'static> {
    ObjectOptions { on_null, keys, ..ObjectOptions::IMPLICIT }
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.34 SR6a, SR6b, SR2b] the
/// implicit clauses: NULL ON NULL, WITHOUT UNIQUE KEYS, RETURNING
/// CHARACTER VARYING of the most characters (ID098).
#[test]
fn the_implicit_clauses() {
    let implicit = ObjectOptions::IMPLICIT;
    assert_eq!(implicit.on_null, OnNull::Null);
    assert_eq!(implicit.keys, Keys::Any);
    assert_eq!(*implicit.returning, IMPLICIT_RETURNING);
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.34 GR2d i, GR2d ii 1,
/// GR5a] the JSON text of an object of the members given, in order: none
/// of none; a string, a number, a truth value, a datetime cast to a
/// string, a FORMAT JSON value parsed, a string not parsed.
#[test]
fn an_object_of_the_members_given() {
    assert_eq!(object_text(&[], &ObjectOptions::IMPLICIT), "{}");
    let names = ["a", "b", "c", "d", "e"].map(text);
    let number = Value::Exact(Decimal::new(-15_i128, 1_u32));
    let date = cast(text("2020-01-02"), &DataType::Date).unwrap();
    let parsed = text(r#"{"x": [1, true]}"#);
    let not_parsed = text("[1]");
    let pairs = [
        (&names[0], plain(&number)),
        (&names[1], plain(&Value::Bool(false))),
        (&names[2], plain(&date)),
        (&names[3], formatted(&parsed)),
        (&names[4], plain(&not_parsed)),
    ];
    assert_eq!(object_text(&pairs, &ObjectOptions::IMPLICIT), r#"{"a":-1.5,"b":false,"c":"2020-01-02","d":{"x":[1,true]},"e":"[1]"}"#);
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.34 GR2d ii 1 F I, GR2d ii
/// 3] NULL ON NULL, the default, keeps a member bound to the SQL/JSON
/// null, of the null value or of FORMAT JSON null; ABSENT ON NULL leaves
/// it out. MC/DC of ABSENT ON NULL and the SQL/JSON null in `add`: null
/// left out ABSENT ON NULL, kept NULL ON NULL; `"1"` kept ABSENT ON NULL.
#[test]
fn the_null_clause() {
    let (a, b, c) = (text("a"), text("b"), text("c"));
    let one = text("1");
    let null = text("null");
    let pairs = [(&a, plain(&Value::Null)), (&b, plain(&one)), (&c, formatted(&null))];
    assert_eq!(object_text(&pairs, &ObjectOptions::IMPLICIT), r#"{"a":null,"b":"1","c":null}"#);
    assert_eq!(object_text(&pairs, &of(OnNull::Absent, Keys::Any)), r#"{"b":"1"}"#);
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.34 SR6b, GR2d ii 2] WITHOUT
/// UNIQUE KEYS, the default, keeps two members of one key; WITH UNIQUE
/// KEYS is SQLSTATE 22030 of them, one left out ABSENT ON NULL too, and
/// keeps keys that differ; the error names the first key given twice.
/// MC/DC of `!fresh && no key noted` in `note`: of a, b, b, a, the first
/// b is fresh, the second noted, and the second a, though not fresh, is
/// not noted over it.
#[test]
fn the_key_uniqueness_constraint() {
    let a = text("a");
    let (one, two) = (Value::Bool(true), Value::Bool(false));
    let twice = [(&a, plain(&one)), (&a, plain(&two))];
    assert_eq!(object_text(&twice, &ObjectOptions::IMPLICIT), r#"{"a":true,"a":false}"#);
    for on_null in [OnNull::Null, OnNull::Absent] {
        let e = object(&twice, &of(on_null, Keys::Unique)).unwrap_err();
        assert_eq!(e.sqlstate().as_str(), "22030");
        assert_eq!(e.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §6.34 GR2d ii 2"));
    }
    let absent = [(&a, plain(&Value::Null)), (&a, plain(&one))];
    assert_eq!(object(&absent, &of(OnNull::Absent, Keys::Unique)).unwrap_err().sqlstate().as_str(), "22030");
    let b = text("b");
    assert_eq!(object_text(&[(&a, plain(&one)), (&b, plain(&two))], &of(OnNull::Null, Keys::Unique)), r#"{"a":true,"b":false}"#);
    let crossed = [(&a, plain(&one)), (&b, plain(&one)), (&b, plain(&two)), (&a, plain(&two))];
    let first = object(&crossed, &of(OnNull::Null, Keys::Unique)).unwrap_err();
    assert!(first.msg.ends_with("the key \"b\""), "{}", first.msg);
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.34 GR2d ii 1 F III] a
/// FORMAT JSON value is parsed under the object's key uniqueness
/// constraint: of two members of one key, SQLSTATE 22037 WITH UNIQUE
/// KEYS alone; one that is no JSON text 22032.
#[test]
fn a_format_json_value_is_parsed_under_wuk() {
    let a = text("a");
    let inner = text(r#"{"k": 1, "k": 2}"#);
    assert_eq!(object_text(&[(&a, formatted(&inner))], &ObjectOptions::IMPLICIT), r#"{"a":{"k":1,"k":2}}"#);
    assert_eq!(object(&[(&a, formatted(&inner))], &of(OnNull::Null, Keys::Unique)).unwrap_err().sqlstate().as_str(), "22037");
    let broken = text("{");
    assert_eq!(object(&[(&a, formatted(&broken))], &ObjectOptions::IMPLICIT).unwrap_err().sqlstate().as_str(), "22032");
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.34 GR2d ii 1 D, GR2d ii 2]
/// a null name is SQLSTATE 22004, whatever its value; the members are
/// taken in turn, so a member's error before it is raised first, and a
/// null name before two keys that are one.
#[test]
fn a_null_name_is_not_allowed() {
    let a = text("a");
    let one = Value::Bool(true);
    assert_eq!(object(&[(&Value::Null, plain(&Value::Null))], &ObjectOptions::IMPLICIT).unwrap_err().sqlstate().as_str(), "22004");
    let broken = text("[");
    let first = [(&a, formatted(&broken)), (&Value::Null, plain(&one))];
    assert_eq!(object(&first, &ObjectOptions::IMPLICIT).unwrap_err().sqlstate().as_str(), "22032");
    let twice_then_null = [(&a, plain(&one)), (&a, plain(&one)), (&Value::Null, plain(&one))];
    assert_eq!(object(&twice_then_null, &of(OnNull::Null, Keys::Unique)).unwrap_err().sqlstate().as_str(), "22004");
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.34 SR4, GR5a, §9.43 GR2a
/// iii] the text is a value of the type returned: CHARACTER padded,
/// CHARACTER LARGE OBJECT; one longer than the type SQLSTATE 22001; a
/// type no string's SQLCODE -103, a binary string type's -900, before any
/// member is read.
#[test]
fn the_type_returned() {
    let a = text("a");
    let one = Value::Bool(true);
    let pairs = [(&a, plain(&one))];
    let padded = object(&pairs, &ObjectOptions { returning: &DataType::character(12), ..ObjectOptions::IMPLICIT }).unwrap();
    assert_eq!(json(&padded), r#"{"a":true}  "#);
    let clob = DataType::Lob { kind: LobKind::Character, length: 100_u64, units: CharUnits::Characters };
    let large = object(&pairs, &ObjectOptions { returning: &clob, ..ObjectOptions::IMPLICIT }).unwrap();
    assert!(matches!(&large, Value::Lob(lob) if lob.kind() == LobKind::Character), "{large:?}");
    let short = object(&pairs, &ObjectOptions { returning: &DataType::varying(9), ..ObjectOptions::IMPLICIT }).unwrap_err();
    assert_eq!(short.sqlstate().as_str(), "22001");
    let null_name = [(&Value::Null, plain(&one))];
    assert_eq!(
        object(&null_name, &ObjectOptions { returning: &DataType::Boolean, ..ObjectOptions::IMPLICIT }).unwrap_err().sqlcode(),
        -103_i32
    );
    let binary = DataType::Lob { kind: LobKind::Binary, length: 100_u64, units: CharUnits::Octets };
    assert_eq!(object(&null_name, &ObjectOptions { returning: &binary, ..ObjectOptions::IMPLICIT }).unwrap_err().sqlcode(), -900_i32);
    let lob_name = Value::Lob(Lob::of_text("k"));
    assert_eq!(object_text(&[(&lob_name, plain(&one))], &ObjectOptions::IMPLICIT), r#"{"k":true}"#);
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.34 GR5a i, §9.43 GR2a ii]
/// an approximate number that is not finite has no JSON text: SQLSTATE
/// 22032.
#[test]
fn a_number_not_finite_has_no_text() {
    let a = text("a");
    let infinite = Value::Approx(f64::INFINITY);
    assert_eq!(object(&[(&a, plain(&infinite))], &ObjectOptions::IMPLICIT).unwrap_err().sqlstate().as_str(), "22032");
}
