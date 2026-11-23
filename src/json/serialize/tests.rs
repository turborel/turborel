//! Tests of serializing an SQL/JSON item: the text written, which parses
//! back to an equivalent item, and the items of which none is.

use super::*;
use crate::json::tests::{nested, number, object, string, truth};
use crate::json::{Keys, Scalar, parse};
use crate::value::lob::LobKind;
use crate::value::{DecFloat, Spelled, Value};

/// `text` with each `%` a reverse solidus and `u`, the start of an escape
/// of four hexadecimal digits.
fn escapes(text: &str) -> String {
    text.replace('%', "\\u")
}

fn text(item: &Item) -> String {
    serialize(item).unwrap()
}

/// The item the JSON text `text` is.
fn json(text: &str) -> Item {
    parse(text, Keys::Any).unwrap()
}

fn scalar(value: Value) -> Item {
    Item::Scalar(Scalar::new(value).unwrap())
}

fn date() -> Item {
    scalar(cast(Value::Char("2020-01-02".into()), &DataType::Date).unwrap())
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.43 GR2a ii] [RFC 8259
/// §3, §4, §5] the literal names, and arrays and objects with no
/// insignificant white space, members in the order kept, those of one key
/// all.
#[test]
fn literal_names_arrays_and_objects() {
    assert_eq!(text(&Item::Null), "null");
    assert_eq!(text(&truth(true)), "true");
    assert_eq!(text(&truth(false)), "false");
    assert_eq!(text(&Item::Array(Vec::new())), "[]");
    assert_eq!(text(&object(&[])), "{}");
    assert_eq!(text(&json(" [ 1 , [ ] , { } , null ] ")), "[1,[],{},null]");
    assert_eq!(text(&json(r#"{ "b" : 1 , "a" : [ true ] , "b" : 2 }"#)), r#"{"b":1,"a":[true],"b":2}"#);
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.43 GR2a ii, §6.13 GR11a,
/// GR12b] an exact number as CAST writes it, its scale kept; an
/// approximate one as its shortest approximate literal, 0E0 of zero; a
/// decimal floating-point one plain, without an exponent, small and large.
#[test]
fn numbers_are_written_as_cast_writes_them() {
    assert_eq!(text(&number(-150, 2)), "-1.50");
    assert_eq!(text(&number(12, 0)), "12");
    assert_eq!(text(&number(5, 3)), "0.005");
    assert_eq!(text(&scalar(Value::Approx(1.5_f64))), "1.5E0");
    assert_eq!(text(&scalar(Value::Approx(0.0_f64))), "0E0");
    assert_eq!(text(&scalar(Value::Approx(-1.0e-3_f64))), "-1.0E-3");
    let large = DecFloat::new(15_i128, 40_i64, 34_u32).unwrap();
    assert_eq!(text(&scalar(Value::DecFloat(large))), format!("15{}", "0".repeat(40)));
    let small = DecFloat::new(-15_i128, -9_i64, 34_u32).unwrap();
    assert_eq!(text(&scalar(Value::DecFloat(small))), "-0.000000015");
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.43 GR2a ii] [RFC 8259
/// §7] a string's quotation marks, reverse solidi and control characters
/// are escaped, each control character of a two-character escape so,
/// the others by four lower case hexadecimal digits; nothing else is, a
/// solidus, DEL, a line separator and a character beyond the Basic
/// Multilingual Plane neither.
#[test]
fn strings_escape_what_rfc_8259_makes_escaped() {
    assert_eq!(text(&string("a\"b\\c/d")), r#""a\"b\\c/d""#);
    assert_eq!(text(&string("\u{8}\u{C}\n\r\t")), r#""\b\f\n\r\t""#);
    assert_eq!(text(&string("\0\u{1}\u{1B}\u{1F}")), escapes(r#""%0000%0001%001b%001f""#));
    assert_eq!(text(&string(" \u{7F}\u{2028}é\u{1D11E}")), "\" \u{7F}\u{2028}é\u{1D11E}\"");
    assert_eq!(text(&object(&[("k\"\n", string("v"))])), r#"{"k\"\n":"v"}"#);
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.43 GR2a ii 2] the text of
/// an item parsed from JSON text parses, WITHOUT UNIQUE KEYS, to an item
/// equivalent to it: of each kind of item, a number of too many digits
/// for an exact number, so decimal floating-point, and escaped characters
/// too.
#[test]
fn the_text_of_an_item_parses_back_to_an_equivalent_one() {
    let digits = "1234567890123456789012345678901234";
    for source in [
        "null".to_owned(),
        "true".to_owned(),
        "[1, -2.50, 3e2, -0.0E-1, \"x\"]".to_owned(),
        r#"{"a": {"b": [{}, []]}, "a": 1, "": ""}"#.to_owned(),
        format!("[{digits}, -0.{digits}, {digits}e-3]"),
        escapes(r#"["%0000 %D834%DD1E \" \\ \/ \b \f \n \r \t %00e9 %2028"]"#),
    ] {
        let item = json(&source);
        let back = json(&text(&item));
        assert!(back.equivalent(&item).unwrap(), "{source} as {}", text(&item));
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.43 GR2a ii] no JSON text
/// parses to a datetime or an approximate number that is not finite, so
/// an item that is or holds one is refused with SQLSTATE 22032.
#[test]
fn a_datetime_or_a_number_not_finite_has_no_text() {
    for item in [
        date(),
        Item::Array(vec![number(1, 0), date()]),
        object(&[("d", date())]),
        scalar(Value::Approx(f64::INFINITY)),
        scalar(Value::Approx(f64::NAN)),
    ] {
        let e = serialize(&item).unwrap_err();
        assert_eq!(e.sqlstate().as_str(), "22032", "{item:?}");
        assert_eq!(e.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.43 GR2a ii"));
    }
}

/// Verifies: [DESIGN] RULES.md R-CF-6: an item nested `MAX_NESTING` deep
/// is serialized, in arrays and in objects; one level deeper is refused
/// with SQLCODE -900.
#[test]
fn serializing_is_bounded_by_the_nesting_limit() {
    let deepest = nested(Item::Null, MAX_NESTING);
    assert_eq!(text(&deepest), format!("{}null{}", "[".repeat(MAX_NESTING), "]".repeat(MAX_NESTING)));
    assert!(serialize(&object(&[("k", nested(Item::Null, MAX_NESTING - 1))])).is_ok());
    assert_eq!(serialize(&nested(Item::Null, MAX_NESTING + 1)).unwrap_err().sqlcode(), -900_i32);
    assert_eq!(serialize(&object(&[("k", nested(Item::Null, MAX_NESTING))])).unwrap_err().sqlcode(), -900_i32);
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.43 GR2a iii] the text as
/// a value of a character string type: of CHARACTER VARYING as it is, of
/// CHARACTER padded with spaces, of CHARACTER LARGE OBJECT a large
/// object; of one character more than the length, SQLSTATE 22001, its
/// length counted in characters or octets as the type's is.
#[test]
fn the_text_as_a_value_of_a_character_string_type() {
    let item = json(r#"{"é":1}"#);
    assert!(matches!(serialize_as(&item, &DataType::varying(7)).unwrap(), Value::Char(s) if s == r#"{"é":1}"#));
    assert!(matches!(serialize_as(&item, &DataType::character(9)).unwrap(), Value::Char(s) if s == "{\"é\":1}  "));
    let clob = DataType::Lob { kind: LobKind::Character, length: 7_u64, units: CharUnits::Characters };
    assert!(matches!(serialize_as(&item, &clob).unwrap(), Value::Lob(l) if l.chars() == 7_u64));
    let e = serialize_as(&item, &DataType::varying(6)).unwrap_err();
    assert_eq!(e.sqlstate().as_str(), "22001");
    assert_eq!(e.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.43 GR2a iii"));
    let octets = DataType::Varying { length: 7, units: CharUnits::Octets };
    assert_eq!(serialize_as(&item, &octets).unwrap_err().sqlstate().as_str(), "22001");
    let short_clob = DataType::Lob { kind: LobKind::Character, length: 6_u64, units: CharUnits::Characters };
    assert_eq!(serialize_as(&item, &short_clob).unwrap_err().sqlstate().as_str(), "22001");
    assert_eq!(serialize_as(&date(), &DataType::varying(8)).unwrap_err().sqlstate().as_str(), "22032");
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.43 SR2a] JSON text is
/// serialized as a character or binary string type: a binary one, whose
/// encodings turborel does not implement, is refused with SQLCODE -900,
/// another type with SQLCODE -103.
#[test]
fn the_target_is_a_string_type() {
    let blob = DataType::Lob { kind: LobKind::Binary, length: 9_u64, units: CharUnits::Characters };
    for target in [DataType::Binary { length: 9 }, DataType::VarBinary { length: 9 }, blob] {
        assert_eq!(serialize_as(&Item::Null, &target).unwrap_err().sqlcode(), -900_i32, "{target}");
        assert_eq!(string_type(&target).unwrap_err().sqlcode(), -900_i32, "{target}");
    }
    for target in [DataType::Boolean, DataType::Date, DataType::Approximate { precision: 53, spelled: Spelled::Float }] {
        let e = serialize_as(&Item::Null, &target).unwrap_err();
        assert_eq!(e.sqlcode(), -103_i32, "{target}");
        assert_eq!(e.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.43 SR2a"));
    }
    assert!(string_type(&DataType::varying(1)).is_ok());
}
