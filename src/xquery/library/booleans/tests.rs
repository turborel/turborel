//! Tests of booleans and effective boolean values.

use super::super::testing::{atom, call, code, dbl, document, empty, int, one, s, seq, text};
use super::*;

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §9.1.1.1,
/// §9.1.2.1, §9.3.1.1] the examples of `fn:true`, `fn:false` and
/// `fn:not`.
#[test]
fn constants_and_negation() {
    assert_eq!(text(call("true", &[])), "true");
    assert_eq!(text(call("false", &[])), "false");
    assert_eq!(text(call("not", &[call("true", &[]).unwrap()])), "false");
    assert_eq!(text(call("not", &[s("false")])), "false");
    assert_eq!(text(call("not", &[empty()])), "true");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §9.2.1 to
/// §9.2.3] the operators on booleans, false less than true; an operand
/// of another type err:XPTY0004.
#[test]
fn boolean_operators() {
    let (t, f) = (Atomic::new(AtomicType::Boolean, "true"), Atomic::new(AtomicType::Boolean, "false"));
    assert!(boolean_equal(&t, &t).unwrap() && !boolean_equal(&t, &f).unwrap());
    assert!(boolean_less_than(&f, &t).unwrap() && !boolean_less_than(&t, &f).unwrap() && !boolean_less_than(&t, &t).unwrap());
    assert!(boolean_greater_than(&t, &f).unwrap() && !boolean_greater_than(&f, &t).unwrap());
    assert!(boolean_equal(&Atomic::new(AtomicType::UntypedAtomic, "1"), &t).unwrap());
    assert!(boolean_equal(&Atomic::new(AtomicType::Integer, "1"), &t).is_err());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §15.1.1.1]
/// the effective boolean value: false of the empty sequence, true of a
/// node first, a boolean itself, a string of whether it is zero-length, a
/// number of whether it is zero or NaN; another sequence err:FORG0006, as
/// the example's three strings are.
#[test]
fn effective_boolean_values() {
    let doc = document("<a/>");
    let cases = [
        (empty(), "false"),
        (seq(&[one(&doc), int(0)]), "true"),
        (atom(AtomicType::Boolean, "false"), "false"),
        (s("false"), "true"),
        (s(""), "false"),
        (atom(AtomicType::UntypedAtomic, ""), "false"),
        (atom(AtomicType::AnyUri, "x"), "true"),
        (int(0), "false"),
        (atom(AtomicType::Decimal, "0.5"), "true"),
        (dbl("NaN"), "false"),
        (dbl("-0"), "false"),
        (dbl("INF"), "true"),
        (atom(AtomicType::Short, "-3"), "true"),
    ];
    for (value, expected) in cases {
        assert_eq!(text(call("boolean", std::slice::from_ref(&value))), expected, "{value:?}");
    }
    assert_eq!(code(call("boolean", &[seq(&[s("a"), s("b"), s("c")])])), "FORG0006");
    assert_eq!(code(call("boolean", &[atom(AtomicType::Date, "2000-01-01")])), "FORG0006");
    assert_eq!(code(call("not", &[seq(&[int(1), one(&doc)])])), "FORG0006");
}
