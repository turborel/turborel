//! Tests of the signs and arithmetic operators, lax and strict.

use crate::json::path::eval::tests::{error, gives, state, value};
use crate::value::Value;

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 GR11h] a sign applies
/// to each item, each a number: `-` negates, `+` gives them as they are;
/// lax: the base is unwrapped; an empty sequence gives nothing.
#[test]
fn a_sign_applies_to_each_number() {
    gives("lax -$", "[1, -2.5, 3e0]", "[-1, 2.5, -3]");
    gives("lax +$", "[1, 2]", "[1, 2]");
    gives("strict -$[*]", "[1, 2]", "[-1, -2]");
    gives("strict -1", "0", "[-1]");
    gives("lax -$.none", "{}", "[]");
    gives("lax - -1", "0", "[1]");
    assert!(matches!(value("lax -$", "1.5e0"), Value::Approx(x) if x.to_bits() == (-1.5_f64).to_bits()));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 GR11h ii] an item of
/// a sign's operand that is no number is SQLSTATE 2203B: a string, null, a
/// truth value; strict: an array, not unwrapped; lax: an array within an
/// array.
#[test]
fn a_sign_of_what_is_no_number_is_an_error() {
    for (path, context) in
        [("lax -$", "\"1\""), ("lax -$", "null"), ("lax +$", "true"), ("strict -$", "[1]"), ("lax -$", "[[1]]"), ("lax +$", "[1, {}]")]
    {
        assert_eq!(state(path, context), "2203B", "{path} of {context}");
    }
    assert_eq!(error("strict -$", "{}").rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.46 GR11h ii"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 GR11i iii 2] `*`, `/`
/// and `%` of singleton numbers, as SQL computes them: exact of exact,
/// approximate if either is; lax: each operand unwrapped first.
#[test]
fn multiplicative_operators_compute_as_sql_does() {
    gives("strict 6 * 7", "0", "[42]");
    gives("strict 7 / 2", "0", "[3.5]");
    gives("strict 7 % 3", "0", "[1]");
    gives("strict -7 % 3", "0", "[-1]");
    gives("lax $ * 2", "[4]", "[8]");
    gives("lax $.a * $.b", r#"{"a": [2], "b": 3}"#, "[6]");
    gives("strict $[0] * $[1]", "[1.5, 2]", "[3]");
    assert!(matches!(value("strict 2e0 * 3", "0"), Value::Approx(x) if x.to_bits() == 6.0_f64.to_bits()));
    assert!(matches!(value("strict 6 / 3", "0"), Value::Exact(_)));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 GR11i iii 1] an
/// operand of `*`, `/` or `%` that is not a singleton numeric value is
/// SQLSTATE 22038: empty, of two items, a string; strict: an array of one.
#[test]
fn multiplicative_operands_are_singleton_numbers() {
    for (path, context) in [
        ("lax $ * 2", "[1, 2]"),
        ("lax $.none * 2", "{}"),
        ("lax 2 / \"1\"", "0"),
        ("strict $ * 2", "[4]"),
        ("strict 2 % $", "[4]"),
        ("lax null * 1", "0"),
    ] {
        assert_eq!(state(path, context), "22038", "{path} of {context}");
    }
    assert_eq!(error("lax $ * 2", "[]").rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.46 GR11i iii 1"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 GR11i iii 2] an
/// exception of the calculation is the result: division by zero is
/// SQLSTATE 22012, of `/` and of `%`.
#[test]
fn an_arithmetic_exception_is_the_result() {
    assert_eq!(state("lax 1 / 0", "0"), "22012");
    assert_eq!(state("lax 1e0 / 0", "0"), "22012");
    assert_eq!(state("lax 5 % 0", "0"), "22012");
    assert_eq!(state("lax 1e308 * 10", "0"), "22003");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 GR11j] `+` and `-` of
/// singleton numbers, as SQL computes them; as printed, no operand is
/// unwrapped in lax mode, so an array of one is SQLSTATE 22038 either way.
#[test]
fn additive_operators_do_not_unwrap() {
    gives("lax 1 + 2", "0", "[3]");
    gives("strict 5 - 7", "0", "[-2]");
    gives("lax $[0] + $[1]", "[1, 2.5]", "[3.5]");
    gives("lax 1 - 2 - 3", "0", "[-4]");
    gives("lax 1 + 2 * 3", "0", "[7]");
    for (path, context) in
        [("lax $ + 1", "[1]"), ("strict $ - 1", "[1]"), ("lax $[*] + 1", "[1, 2]"), ("lax \"a\" + 1", "0"), ("lax 1 - $.none", "{}")]
    {
        assert_eq!(state(path, context), "22038", "{path} of {context}");
    }
    assert_eq!(error("lax $ + 1", "[1]").rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.46 GR11j ii 1"));
    assert_eq!(state("lax 1e308 + 1e308", "0"), "22003");
}
