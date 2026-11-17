//! Tests of parsing JSON text: what is one, and the item it is.

use super::*;
use crate::json::tests::{nested, number, object, string, truth};

fn item(text: &str) -> Item {
    parse(text, Keys::Any).unwrap()
}

/// Whether `text` is the JSON text of an item equivalent to `expected`.
fn is(text: &str, expected: &Item) -> bool {
    item(text).equivalent(expected).unwrap()
}

/// The SQLSTATE `text` is refused with, or none.
fn state(text: &str) -> String {
    parse(text, Keys::Any).map_or_else(|e| e.sqlstate().as_str().to_owned(), |_| String::new())
}

/// `text` with each `%` a reverse solidus and `u`, the start of an escape
/// of four hexadecimal digits.
fn escapes(text: &str) -> String {
    text.replace('%', "\\u")
}

/// The bits of the approximate number `text` is, if it is one.
fn bits(text: &str) -> Option<u64> {
    if let Value::Approx(x) = value(text) { Some(x.to_bits()) } else { None }
}

fn value(text: &str) -> Value {
    item(text).as_scalar().unwrap().value().clone()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.42 GR3a iii 3 A I to III]
/// [RFC 8259 §3] false and true are the truth values, null the SQL/JSON
/// null; a name of another case, cut short or longer is no JSON text.
#[test]
fn the_literal_names() {
    assert!(is("true", &truth(true)));
    assert!(is("false", &truth(false)));
    assert!(matches!(item("null"), Item::Null));
    for text in ["True", "tru", "nul", "fals", "truee", "nulll", "none"] {
        assert_eq!(state(text), "22032", "{text}");
    }
}

/// Verifies: [EXTENSION] [RFC 8259 §2] `JSON-text = ws value ws`: spaces,
/// tabs, line feeds and carriage returns around a value and around the
/// structural characters are insignificant; an empty text, a second value
/// and another white space character are no JSON text.
#[test]
fn white_space_is_insignificant() {
    assert!(is(" \t\r\n1 \t\r\n", &number(1, 0)));
    assert!(is(" [ 1 , 2 ] ", &Item::Array(vec![number(1, 0), number(2, 0)])));
    assert!(is(" { \"a\" : 1 , \"b\" : 2 } ", &object(&[("a", number(1, 0)), ("b", number(2, 0))])));
    for text in ["", "  ", "1 2", "1,", "\u{B}1", "\u{A0}1"] {
        assert_eq!(state(text), "22032", "{text:?}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.42 GR3a iii 3 A IV] [RFC
/// 8259 §6] a number without an exponent is the exact number its
/// characters are of a `<signed numeric literal>`, its scale its
/// fraction's digits.
#[test]
fn a_number_without_an_exponent_is_exact() {
    assert!(is("0", &number(0, 0)));
    assert!(is("-0", &number(0, 0)));
    assert!(is("12", &number(12, 0)));
    assert!(is("-1.50", &number(-150, 2)));
    assert!(is("0.001", &number(1, 3)));
    let Value::Exact(kept) = value("-1.50") else { panic!("an exact number") };
    assert_eq!((kept.units, kept.scale), (-150_i128, 2_u32));
    assert!(is("123456789012345678", &number(123_456_789_012_345_678, 0)));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.42 GR3a iii 3 A IV] [RFC
/// 8259 §6] a number with an exponent is the approximate number of its
/// characters, the exponent signed or not, E of either case; one past an
/// approximate number's range is SQLCODE -900.
#[test]
fn a_number_with_an_exponent_is_approximate() {
    assert_eq!(bits("1e2"), Some(100.0_f64.to_bits()));
    assert_eq!(bits("-1.5E+3"), Some((-1500.0_f64).to_bits()));
    assert_eq!(bits("25e-1"), Some(2.5_f64.to_bits()));
    assert_eq!(bits("0E0"), Some(0.0_f64.to_bits()));
    assert!(is("1e2", &number(100, 0)));
    assert_eq!(parse("1E400", Keys::Any).unwrap_err().sqlcode(), -900_i32);
}

/// Verifies: [DESIGN] an exact number of more digits than
/// `MAX_EXACT_PRECISION` is a decimal floating-point number, rounded to
/// `MAX_DECFLOAT_PRECISION` digits, as RFC 8259 §6 lets precision be
/// limited; one past its range is SQLCODE -900.
#[test]
fn an_exact_number_of_many_digits_is_decimal_floating_point() {
    let held = DecFloat::of_digits(false, "1234567890123456789", 0, MAX_DECFLOAT_PRECISION).unwrap();
    assert!(matches!(value("1234567890123456789"), Value::DecFloat(d) if d == held));
    let small = DecFloat::of_digits(true, "1", -21, MAX_DECFLOAT_PRECISION).unwrap();
    assert!(matches!(value("-0.000000000000000000001"), Value::DecFloat(d) if d == small));
    let rounded = DecFloat::of_digits(false, "1", 39, MAX_DECFLOAT_PRECISION).unwrap();
    assert!(matches!(value(&format!("1{}1", "0".repeat(38))), Value::DecFloat(d) if d == rounded));
    let beyond = format!("1{}", "0".repeat(7000));
    assert_eq!(parse(&beyond, Keys::Any).unwrap_err().sqlcode(), -900_i32);
}

/// Verifies: [EXTENSION] [RFC 8259 §6] what is no number: a leading zero,
/// a plus sign, no digit before or after the decimal point or in the
/// exponent, a hexadecimal one, Infinity and NaN.
#[test]
fn what_is_no_number() {
    for text in ["01", "-01", "+1", "-", ".5", "1.", "1.e2", "1e", "1e+", "1E-", "0x10", "Infinity", "NaN", "-a", "1_000"] {
        assert_eq!(state(text), "22032", "{text}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.42 GR3a iii 3 A V] [RFC
/// 8259 §7] a string is its characters with each escape replaced: the
/// two-character escapes, `\u` of four hexadecimal digits of either case,
/// and two of them that are a surrogate pair, one character.
#[test]
fn a_string_is_its_characters_unescaped() {
    assert!(is("\"\"", &string("")));
    assert!(is("\"a b\"", &string("a b")));
    assert!(is(r#""\"\\\/\b\f\n\r\t""#, &string("\"\\/\u{8}\u{C}\n\r\t")));
    assert!(is(&escapes("\"%0041%00e9%00E9%005C\""), &string("Aéé\\")));
    assert!(is(&escapes("\"%D834%DD1E\""), &string("\u{1D11E}")));
    assert!(is(&escapes("\"%DBFF%DFFF\""), &string("\u{10FFFF}")));
    assert!(is("\"é\u{1D11E}\u{7F}\"", &string("é\u{1D11E}\u{7F}")));
    let Value::Char(kept) = value(&escapes("\"a%0000b\"")) else { panic!("a character string") };
    assert_eq!(kept, "a\0b");
}

/// Verifies: [EXTENSION] [RFC 8259 §7] what is no string: one not closed,
/// a control character not escaped, an escape of another character or cut
/// short, a `\u` of fewer than four hexadecimal digits, a single
/// quotation mark; \[DESIGN\] and a surrogate that is not of a pair, no
/// Unicode character.
#[test]
fn what_is_no_string() {
    let texts = [
        "\"a",
        "\"a\\",
        "\"a\nb\"",
        "\"\u{1F}\"",
        "\"\\a\"",
        "\"%12\"",
        "\"%12G4\"",
        "\"%004",
        "'a'",
        "\"%D834\"",
        "\"%D834x\"",
        "\"%D834\\n\"",
        "\"%D834%0041\"",
        "\"%D834%D834\"",
        "\"%DD1E\"",
        "\"%D834%DD1\"",
    ];
    for text in texts {
        assert_eq!(state(&escapes(text)), "22032", "{text}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.42 GR3a iii 3 A VI] [RFC
/// 8259 §5] an array is its elements in order, of any types, nested or
/// none; one not closed, with a comma before no element or with elements
/// not separated is no JSON text.
#[test]
fn an_array_is_its_elements_in_order() {
    assert!(is("[]", &Item::Array(Vec::new())));
    assert!(is("[1,\"a\",null,true]", &Item::Array(vec![number(1, 0), string("a"), Item::Null, truth(true)])));
    assert!(is("[[1],[]]", &Item::Array(vec![Item::Array(vec![number(1, 0)]), Item::Array(Vec::new())])));
    assert!(!is("[1,2]", &Item::Array(vec![number(2, 0), number(1, 0)])));
    for text in ["[", "[1", "[1,", "[1,]", "[,1]", "[1 2]", "[1;2]", "]", "[1]]"] {
        assert_eq!(state(text), "22032", "{text}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.42 GR3a iii 3 A VII,
/// VIII] [RFC 8259 §4] an object is its members, each a string key and a
/// value; a key that is no string, a member without its colon or value,
/// a comma before no member and an object not closed are no JSON text.
#[test]
fn an_object_is_its_members() {
    assert!(is("{}", &object(&[])));
    assert!(is("{\"a\":1,\"b\":{\"c\":[]}}", &object(&[("a", number(1, 0)), ("b", object(&[("c", Item::Array(Vec::new()))]))])));
    assert!(is("{\"\":null}", &object(&[("", Item::Null)])));
    assert!(is(&escapes("{\"%0061\":1}"), &object(&[("a", number(1, 0))])));
    for text in ["{", "{\"a\"", "{\"a\":", "{\"a\":1", "{\"a\":1,", "{\"a\":1,}", "{a:1}", "{1:1}", "{\"a\" 1}", "{\"a\":1 \"b\":2}", "}"] {
        assert_eq!(state(text), "22032", "{text}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.42 GR3a iii 2, GR3a iii 3
/// A VIII 2] members of one key are all kept, in the order written (UA057),
/// unless keys are to be unique: then SQLSTATE 22037, in a nested object
/// too, keys compared after their escapes are replaced; one key in two
/// objects is unique in each.
#[test]
fn members_of_one_key_are_kept_or_refused() {
    let twice = item("{\"a\":1,\"a\":2}");
    let members = twice.as_object().unwrap();
    assert_eq!(members.iter().map(|m| m.key.as_str()).collect::<Vec<_>>(), ["a", "a"]);
    assert!(members.first().unwrap().value.equivalent(&number(1, 0)).unwrap());
    let unique = |text: &str| parse(text, Keys::Unique).map_or_else(|e| e.sqlstate().as_str().to_owned(), |_| String::new());
    assert_eq!(unique("{\"a\":1,\"a\":2}"), "22037");
    assert_eq!(unique("[{\"b\":{\"a\":1,\"c\":2,\"a\":1}}]"), "22037");
    assert_eq!(unique(&escapes("{\"a\":1,\"%0061\":2}")), "22037");
    assert_eq!(unique("{\"a\":1,\"A\":2}"), "");
    assert_eq!(unique("[{\"a\":1},{\"a\":1}]"), "");
    assert_eq!(unique("{\"a\":{\"a\":1}}"), "");
    assert_eq!(unique("{}"), "");
}

/// Verifies: [DESIGN] RULES.md R-CF-6: arrays and objects nested
/// `MAX_NESTING` deep are parsed, one level deeper SQLCODE -900, whether
/// the deepest is an array or an object.
#[test]
fn nesting_is_bounded() {
    let arrays = |levels: usize| format!("{}{}", "[".repeat(levels), "]".repeat(levels));
    assert!(is(&arrays(MAX_NESTING), &nested(Item::Array(Vec::new()), MAX_NESTING - 1)));
    assert_eq!(parse(&arrays(MAX_NESTING + 1), Keys::Any).unwrap_err().sqlcode(), -900_i32);
    let objects = |levels: usize| format!("{}1{}", "{\"k\":".repeat(levels), "}".repeat(levels));
    assert!(parse(&objects(MAX_NESTING), Keys::Any).is_ok());
    assert_eq!(parse(&objects(MAX_NESTING + 1), Keys::Any).unwrap_err().sqlcode(), -900_i32);
    let mixed = format!("{}{{}}{}", "[".repeat(MAX_NESTING), "]".repeat(MAX_NESTING));
    assert_eq!(parse(&mixed, Keys::Any).unwrap_err().sqlcode(), -900_i32);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.42 GR3a iii 1] the
/// refusal of a string that is no JSON text names its rule and the
/// character it was refused at, as a data exception.
#[test]
fn a_refusal_names_its_rule_and_place() {
    let refused = parse("[1, x]", Keys::Any).unwrap_err();
    assert_eq!(refused.sqlcode(), -301_i32);
    assert_eq!(refused.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.42 GR3a iii 1"));
    assert!(refused.msg.contains("(at character 5)"), "{}", refused.msg);
    let twice = parse("{\"k\":1,\"k\":2}", Keys::Unique).unwrap_err();
    assert_eq!(twice.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.42 GR3a iii 2"));
}
