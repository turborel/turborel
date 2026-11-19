//! Tests of the functions on numbers.

use super::super::testing::{atom, call, code, dbl, dec, empty, int, s, typed};
use crate::xml::AtomicType;

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §6.4.1.1 to
/// §6.4.4.1] the examples of `fn:abs`, `fn:ceiling`, `fn:floor` and
/// `fn:round`, each of the decimal type of its argument.
#[test]
fn examples_of_abs_ceiling_floor_round() {
    let cases = [
        ("abs", "10.5", "10.5"),
        ("abs", "-10.5", "10.5"),
        ("ceiling", "10.5", "11"),
        ("ceiling", "-10.5", "-10"),
        ("floor", "10.5", "10"),
        ("floor", "-10.5", "-11"),
        ("round", "2.5", "3"),
        ("round", "2.4999", "2"),
        ("round", "-2.5", "-2"),
    ];
    for (function, arg, expected) in cases {
        assert_eq!(typed(call(function, &[dec(arg)])), (AtomicType::Decimal, expected.to_owned()), "fn:{function}({arg})");
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §6.4] of a
/// float or double: NaN itself, infinities kept but by `fn:abs`, zeros of
/// their sign; a negative number rounding to zero negative zero; a type
/// derived from a numeric type its base's; an untyped value a double; the
/// empty sequence of the empty sequence; a string err:XPTY0004.
#[test]
fn special_values_and_types() {
    let cases = [
        ("abs", "-0", "0"),
        ("abs", "-INF", "INF"),
        ("ceiling", "-0.5", "-0"),
        ("ceiling", "NaN", "NaN"),
        ("floor", "-0", "-0"),
        ("floor", "INF", "INF"),
        ("round", "-0.5", "-0"),
        ("round", "-0.4", "-0"),
        ("round", "0.49999999999999994", "0"),
        ("round", "-INF", "-INF"),
        ("round", "2.5", "3"),
    ];
    for (function, arg, expected) in cases {
        assert_eq!(typed(call(function, &[dbl(arg)])), (AtomicType::Double, expected.to_owned()), "fn:{function}({arg})");
    }
    assert_eq!(typed(call("abs", &[atom(AtomicType::Short, "-3")])), (AtomicType::Integer, "3".to_owned()));
    assert_eq!(typed(call("floor", &[atom(AtomicType::Float, "1.5")])), (AtomicType::Float, "1".to_owned()));
    assert_eq!(typed(call("round", &[atom(AtomicType::UntypedAtomic, "1.5")])), (AtomicType::Double, "2".to_owned()));
    assert!(call("ceiling", &[empty()]).unwrap().items().is_empty());
    assert_eq!(code(call("abs", &[s("1")])), "XPTY0004");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §6.4.5.1]
/// the examples of `fn:round-half-to-even`, of decimals and doubles, of a
/// precision and of none; a negative precision rounding to tens.
#[test]
fn examples_of_round_half_to_even() {
    assert_eq!(typed(call("round-half-to-even", &[dec("0.5")])).1, "0");
    assert_eq!(typed(call("round-half-to-even", &[dec("1.5")])).1, "2");
    assert_eq!(typed(call("round-half-to-even", &[dec("2.5")])).1, "2");
    assert_eq!(typed(call("round-half-to-even", &[dbl("3.567812E+3"), int(2)])), (AtomicType::Double, "3567.81".to_owned()));
    assert_eq!(typed(call("round-half-to-even", &[dbl("4.7564E-3"), int(2)])).1, "0");
    assert_eq!(typed(call("round-half-to-even", &[dec("35612.25"), int(-2)])).1, "35600");
    assert_eq!(typed(call("round-half-to-even", &[int(35612), int(-2)])), (AtomicType::Integer, "35600".to_owned()));
    assert_eq!(typed(call("round-half-to-even", &[atom(AtomicType::Float, "2.5")])), (AtomicType::Float, "2".to_owned()));
    assert_eq!(typed(call("round-half-to-even", &[dbl("-0.25"), int(0)])).1, "-0");
    assert_eq!(typed(call("round-half-to-even", &[dbl("NaN"), int(1)])).1, "NaN");
    assert_eq!(typed(call("round-half-to-even", &[dec("1.25"), int(1_000_000)])).1, "1.25");
    assert_eq!(typed(call("round-half-to-even", &[dec("1.25"), atom(AtomicType::Integer, "-99999999999999999999999")])).1, "0");
    assert!(call("round-half-to-even", &[empty(), int(1)]).unwrap().items().is_empty());
    assert_eq!(code(call("round-half-to-even", &[dec("1"), dec("1")])), "XPTY0004");
}
