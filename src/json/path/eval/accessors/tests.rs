//! Tests of the accessor ops, lax and strict: members, wildcards,
//! subscripts and their ranges, and filters.

use super::*;
use crate::json::path::eval::tests::{error, gives, run, state};
use crate::value::Decimal;

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 GR11g ii 1] lax: the
/// base is unwrapped, and of each object the bound value of the key; an
/// item that is no object, or one without the key, gives nothing.
#[test]
fn lax_member_accessor_unwraps_and_skips_what_lacks_the_key() {
    gives("lax $.a", r#"{"a": 1, "b": 2}"#, "[1]");
    gives("lax $.a", r#"[{"a": 1}, {"b": 2}, 3, {"a": [4]}]"#, "[1, [4]]");
    gives("lax $.a", "[[{\"a\": 1}]]", "[]");
    gives("lax $.\"a b\"", r#"{"a b": true}"#, "[true]");
    gives("lax $.a", "7", "[]");
    gives("lax $.a.b", r#"{"a": {"b": "x"}}"#, r#"["x"]"#);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 GR11g ii 1 B II]
/// strict: an item that is not an object with a member of the key is
/// SQLSTATE 2203A, one of an array of objects too; and the base is not
/// unwrapped.
#[test]
fn strict_member_accessor_requires_an_object_with_the_key() {
    gives("strict $.a", r#"{"a": 1}"#, "[1]");
    assert_eq!(state("strict $.a", r#"{"b": 1}"#), "2203A");
    assert_eq!(state("strict $.a", "[{\"a\": 1}]"), "2203A");
    assert_eq!(state("strict $.a", "1"), "2203A");
    let e = error("strict $.a", "null");
    assert_eq!(e.sqlcode(), -301_i32);
    assert_eq!(e.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.46 GR11g ii 1 B II"));
    gives("strict $[*].a", r#"[{"a": 1}, {"a": 2}]"#, "[1, 2]");
    assert_eq!(state("strict $[*].a", r#"[{"a": 1}, {"b": 2}]"#), "2203A");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 GR11g ii 1 C, D II 2
/// a] (IA071) an object of two members of the key raises no exception;
/// (UA075) the first is taken.
#[test]
fn member_accessor_takes_the_first_of_duplicate_keys() {
    gives("lax $.a", r#"{"a": 1, "a": 2}"#, "[1]");
    gives("strict $.a", r#"{"b": 0, "a": 2, "a": 1}"#, "[2]");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 GR11g ii 2] lax: the
/// bound values of every member of each object, in the order kept,
/// duplicates all kept (UA058); what is no object gives nothing; the base
/// is unwrapped.
#[test]
fn lax_wildcard_member_accessor_gives_every_bound_value() {
    gives("lax $.*", r#"{"a": 1, "b": [2], "a": 3}"#, "[1, [2], 3]");
    gives("lax $.*", r#"[{"a": 1}, 2, {"b": null}]"#, "[1, null]");
    gives("lax $.*", "{}", "[]");
    gives("lax $.*", "\"s\"", "[]");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 GR11g ii 2 A II]
/// strict: an item that is no object is SQLSTATE 2203C.
#[test]
fn strict_wildcard_member_accessor_requires_objects() {
    gives("strict $.*", r#"{"a": 1, "b": 2}"#, "[1, 2]");
    assert_eq!(state("strict $.*", "[{\"a\": 1}]"), "2203C");
    assert_eq!(state("strict $[*].*", r#"[{"a": 1}, 2]"#), "2203C");
    assert_eq!(error("strict $.*", "1").rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.46 GR11g ii 2 A II"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 GR11g ii 3] the
/// elements subscripts select, 0-relative, of each array in turn; ranges
/// inclusive; the subscripts' selections concatenated in order, a
/// subscript twice selecting twice.
#[test]
fn subscripts_select_elements_in_order() {
    gives("strict $[0]", "[10, 20, 30]", "[10]");
    gives("strict $[2, 0]", "[10, 20, 30]", "[30, 10]");
    gives("strict $[0 to 1, 1]", "[10, 20, 30]", "[10, 20, 20]");
    gives("strict $[1 to 1]", "[10, 20, 30]", "[20]");
    gives("strict $[*][0]", "[[1, 2], [3]]", "[1, 3]");
    gives("strict $[1 + 1]", "[10, 20, 30]", "[30]");
    gives("lax $[0][1]", "[[1, 2]]", "[2]");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 GR11g ii 3 F II 1]
/// (IA073) a fractional subscript is truncated toward zero, exact,
/// approximate and decimal floating-point alike: −0.5 is 0.
#[test]
fn a_fractional_subscript_is_truncated_toward_zero() {
    gives("strict $[1.9]", "[10, 20, 30]", "[20]");
    gives("strict $[1.9e0]", "[10, 20, 30]", "[20]");
    gives("strict $[-0.5]", "[10, 20, 30]", "[10]");
    gives("strict $[-0.5e0]", "[10, 20, 30]", "[10]");
    gives("strict $[2e0]", "[10, 20, 30]", "[30]");
    gives("strict $[0.00000000000000000000000000000000000001]", "[10]", "[10]");
    let d = DecFloat::of_decimal(&Decimal::new(19, 1)).unwrap();
    assert_eq!(position(digits(Number::DecFloat(d)).unwrap(), 3), Position::At(1));
    let big = DecFloat::of_decimal(&Decimal::new(5, 0)).unwrap();
    assert_eq!(position(digits(Number::DecFloat(big)).unwrap(), 3), Position::Beyond);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 GR11g ii 3 F II 2 a]
/// strict: a subscript negative, or not below the length, or a range's
/// first past its last, is SQLSTATE 22033.
#[test]
fn strict_subscripts_outside_the_array_are_errors() {
    for path in
        ["strict $[3]", "strict $[-1]", "strict $[2 to 1]", "strict $[0 to 3]", "strict $[-1 to 0]", "strict $[1e40]", "strict $[-1e40]"]
    {
        assert_eq!(state(path, "[10, 20, 30]"), "22033", "{path}");
    }
    let e = error("strict $[3]", "[1]");
    assert_eq!(e.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.46 GR11g ii 3 F II 2 a"));
    assert_eq!(state("strict $[0]", "[]"), "22033");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 GR11g ii 3 F II 2 b]
/// lax: of a subscript or range, only the elements there are, ascending:
/// none of a range past the array or reversed, the rest of one partly
/// out; a huge range is not walked.
#[test]
fn lax_subscripts_take_only_the_elements_there_are() {
    gives("lax $[3]", "[10, 20, 30]", "[]");
    gives("lax $[-1]", "[10, 20, 30]", "[]");
    gives("lax $[2 to 1]", "[10, 20, 30]", "[]");
    gives("lax $[1 to 5]", "[10, 20, 30]", "[20, 30]");
    gives("lax $[-5 to 0]", "[10, 20, 30]", "[10]");
    gives("lax $[-5 to -1]", "[10, 20, 30]", "[]");
    gives("lax $[3 to 5]", "[10, 20, 30]", "[]");
    gives("lax $[0 to 1e40]", "[10, 20]", "[10, 20]");
    gives("lax $[-1e40 to 1e40]", "[10, 20]", "[10, 20]");
    gives("lax $[0 to 99999999999999999999999999999999999999]", "[10]", "[10]");
    gives("lax $[1e40 to 1e41]", "[10]", "[]");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 GR11g ii 3 A, B] lax:
/// the base is wrapped, so an item that is no array is the array of it
/// alone; strict: it is SQLSTATE 22039, checked of every item before any
/// subscript.
#[test]
fn array_accessor_wraps_in_lax_mode_and_requires_arrays_in_strict() {
    gives("lax $[0]", r#"{"a": 1}"#, r#"[{"a": 1}]"#);
    gives("lax $[0]", "5", "[5]");
    gives("lax $[1]", "5", "[]");
    gives("lax $.a[0]", r#"[{"a": [1]}, {"a": 2}]"#, "[1, 2]");
    assert_eq!(state("strict $[0]", "5"), "22039");
    assert_eq!(state("strict $[*][9]", "[[1], 2]"), "22039");
    assert_eq!(error("strict $[0]", "{}").rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.46 GR11g ii 3 B"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 GR11g ii 3 E III] a
/// subscript that is not a singleton numeric value is SQLSTATE 22033 in
/// either mode: a string, an empty sequence, two items; \[DESIGN\] an
/// exception of the subscript's wff is the result.
#[test]
fn a_subscript_is_a_singleton_number() {
    for mode in ["lax", "strict"] {
        assert_eq!(state(&format!("{mode} $[\"0\"]"), "[1]"), "22033", "{mode}");
        assert_eq!(state(&format!("{mode} $[$[*] ? (@ > 5)]"), "[1]"), "22033", "{mode}");
        assert_eq!(state(&format!("{mode} $[0 to $[*]]"), "[1, 2]"), "22033", "{mode}");
        assert_eq!(state(&format!("{mode} $[null]"), "[1]"), "22033", "{mode}");
        assert_eq!(state(&format!("{mode} $[1 / 0]"), "[1]"), "22012", "{mode}");
    }
    let e = error("lax $[true]", "[1]");
    assert_eq!(e.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.46 GR11g ii 3 E III"));
    assert!(digits(Number::Approx(f64::INFINITY)).is_none());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 GR11g ii 4] every
/// element of each array, in order; lax: an item that is no array is
/// wrapped; strict: it is SQLSTATE 22039.
#[test]
fn wildcard_array_accessor_gives_every_element() {
    gives("strict $[*]", "[1, [2], null]", "[1, [2], null]");
    gives("strict $[*]", "[]", "[]");
    gives("lax $[*]", "7", "[7]");
    gives("lax $.a[*]", r#"[{"a": [1, 2]}, {"a": 3}]"#, "[1, 2, 3]");
    assert_eq!(state("strict $[*]", "7"), "22039");
    assert_eq!(error("strict $[*]", "{}").rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.46 GR11g ii 4 B"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 GR11g ii 5] a filter
/// keeps the items its predicate is True of, in order; lax: the base is
/// unwrapped first; strict: it is not, so an array is tested whole.
#[test]
fn filter_keeps_the_items_its_predicate_is_true_of() {
    gives("lax $ ? (@ > 1)", "[1, 2, 3]", "[2, 3]");
    gives("strict $ ? (@ > 1)", "[1, 2, 3]", "[]");
    gives("strict $[*] ? (@ > 1)", "[1, 2, 3]", "[2, 3]");
    gives("strict $ ? (@.size() == 3)", "[1, 2, 3]", "[[1, 2, 3]]");
    gives("lax $ ? (@ == 9)", "[1, 2]", "[]");
    gives("lax $.a ? (@ == 1)", "{}", "[]");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.48.5, §9.46 GR12] an
/// exception within a filter's predicate is caught, strict mode's
/// structural errors too: the predicate is Unknown, and the item not kept.
#[test]
fn errors_within_a_filter_make_its_predicate_unknown() {
    gives("strict $[*] ? (@.a == 1)", r#"[{"a": 1}, {"b": 1}, 2]"#, r#"[{"a": 1}]"#);
    gives("lax $[*] ? (@ / 0 == 1)", "[1, 2]", "[]");
    gives("lax $[*] ? ((@ / 0 == 1) is unknown)", "[1, 2]", "[1, 2]");
    gives("strict $[*] ? (@[5] == 1)", "[[1]]", "[]");
    assert!(run("lax $ ? (@ == 1)", "[1]").is_ok());
}
