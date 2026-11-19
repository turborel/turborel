//! Tests of the operators on numbers.

use super::*;

fn a(kind: AtomicType, text: &str) -> Atomic {
    Atomic::new(kind, text)
}

fn i(text: &str) -> Atomic {
    a(AtomicType::Integer, text)
}

fn dec(text: &str) -> Atomic {
    a(AtomicType::Decimal, text)
}

fn dbl(text: &str) -> Atomic {
    a(AtomicType::Double, text)
}

fn flt(text: &str) -> Atomic {
    a(AtomicType::Float, text)
}

/// The type and lexical form of `r`.
fn shown(r: Result<Atomic>) -> (AtomicType, String) {
    let v = r.unwrap();
    (v.kind, v.lexical.into_string())
}

/// The error code of `r`.
fn code(r: Result<Atomic>) -> String {
    let message = r.unwrap_err().to_string();
    let (_, after) = message.split_once("err:").unwrap();
    after.chars().take_while(char::is_ascii_alphanumeric).collect()
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §6.2]
/// operands of different types are promoted to one, the result of that
/// type: integer, decimal, float, double; a derived type its base's; two
/// integers divided a decimal; an untyped operand a double.
#[test]
fn operands_are_promoted() {
    assert_eq!(shown(numeric_add(&i("1"), &i("2"))), (AtomicType::Integer, "3".into()));
    assert_eq!(shown(numeric_add(&a(AtomicType::Byte, "1"), &a(AtomicType::Int, "2"))), (AtomicType::Integer, "3".into()));
    assert_eq!(shown(numeric_add(&i("1"), &dec("0.5"))), (AtomicType::Decimal, "1.5".into()));
    assert_eq!(shown(numeric_multiply(&dec("1.5"), &flt("2"))), (AtomicType::Float, "3".into()));
    assert_eq!(shown(numeric_subtract(&flt("1"), &dbl("0.5"))), (AtomicType::Double, "0.5".into()));
    assert_eq!(shown(numeric_divide(&i("1"), &i("4"))), (AtomicType::Decimal, "0.25".into()));
    assert_eq!(shown(numeric_divide(&i("1"), &i("3"))), (AtomicType::Decimal, "0.333333333333333333".into()));
    assert_eq!(shown(numeric_add(&a(AtomicType::UntypedAtomic, "1"), &i("1"))), (AtomicType::Double, "2".into()));
    assert_eq!(code(numeric_add(&a(AtomicType::String, "1"), &i("1"))), "XPTY0004");
    assert_eq!(code(numeric_add(&a(AtomicType::UntypedAtomic, "x"), &i("1"))), "FORG0001");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §6.2.1 to
/// §6.2.4] floats and doubles by IEEE 754: infinities and NaN, division by
/// zero an infinity; a float's result rounded to single precision;
/// integers and decimals divided by zero err:FOAR0001.
#[test]
fn reals_follow_ieee_754() {
    assert_eq!(shown(numeric_add(&dbl("INF"), &dbl("1"))).1, "INF");
    assert_eq!(shown(numeric_add(&dbl("INF"), &dbl("-INF"))).1, "NaN");
    assert_eq!(shown(numeric_subtract(&dbl("INF"), &dbl("INF"))).1, "NaN");
    assert_eq!(shown(numeric_multiply(&dbl("0"), &dbl("INF"))).1, "NaN");
    assert_eq!(shown(numeric_divide(&dbl("1"), &dbl("0"))).1, "INF");
    assert_eq!(shown(numeric_divide(&dbl("-1"), &dbl("0"))).1, "-INF");
    assert_eq!(shown(numeric_divide(&dbl("1"), &dbl("-0"))).1, "-INF");
    assert_eq!(shown(numeric_divide(&dbl("0"), &dbl("0"))).1, "NaN");
    assert_eq!(shown(numeric_add(&flt("0.1"), &flt("0.2"))), (AtomicType::Float, "0.3".into()));
    assert_eq!(shown(numeric_add(&dbl("0.1"), &dbl("0.2"))).1, "0.30000000000000004");
    assert_eq!(shown(numeric_multiply(&dbl("1e308"), &dbl("10"))).1, "INF");
    assert_eq!(code(numeric_divide(&i("1"), &i("0"))), "FOAR0001");
    assert_eq!(code(numeric_divide(&dec("1"), &dec("0.0"))), "FOAR0001");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §6.2.5.1]
/// the examples of `op:numeric-integer-divide`, and its errors: a zero
/// divisor err:FOAR0001, a NaN or infinite dividend err:FOAR0002.
#[test]
fn integer_division_examples() {
    let cases = [
        (i("10"), i("3"), "3"),
        (i("3"), i("-2"), "-1"),
        (i("-3"), i("2"), "-1"),
        (i("-3"), i("-2"), "1"),
        (dec("9.0"), i("3"), "3"),
        (dec("-3.5"), i("3"), "-1"),
        (dec("3.0"), i("4"), "0"),
        (dbl("3.1E1"), i("6"), "5"),
        (dbl("3.1E1"), i("7"), "4"),
        (flt("7.5"), flt("2"), "3"),
    ];
    for (x, y, expected) in cases {
        assert_eq!(shown(numeric_integer_divide(&x, &y)), (AtomicType::Integer, expected.into()), "{x:?} idiv {y:?}");
    }
    assert_eq!(code(numeric_integer_divide(&i("1"), &i("0"))), "FOAR0001");
    assert_eq!(code(numeric_integer_divide(&dbl("1"), &dbl("-0"))), "FOAR0001");
    assert_eq!(code(numeric_integer_divide(&dbl("NaN"), &dbl("1"))), "FOAR0002");
    assert_eq!(code(numeric_integer_divide(&dbl("INF"), &dbl("1"))), "FOAR0002");
    assert_eq!(code(numeric_integer_divide(&dbl("1e308"), &dbl("1e-308"))), "FOAR0002");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §6.2.6.1]
/// the examples of `op:numeric-mod`, the rules of floats: NaN of an
/// infinite dividend or a zero divisor, the dividend of an infinite
/// divisor; a zero divisor of decimals err:FOAR0001.
#[test]
fn modulus_examples() {
    assert_eq!(shown(numeric_mod(&i("10"), &i("3"))).1, "1");
    assert_eq!(shown(numeric_mod(&i("6"), &i("-2"))).1, "0");
    assert_eq!(shown(numeric_mod(&dec("4.5"), &dec("1.2"))).1, "0.9");
    assert_eq!(shown(numeric_mod(&dbl("1.23E2"), &dbl("0.6E1"))), (AtomicType::Double, "3".into()));
    assert_eq!(shown(numeric_mod(&i("-10"), &i("3"))).1, "-1");
    assert_eq!(shown(numeric_mod(&dbl("INF"), &dbl("3"))).1, "NaN");
    assert_eq!(shown(numeric_mod(&dbl("3"), &dbl("0"))).1, "NaN");
    assert_eq!(shown(numeric_mod(&dbl("3"), &dbl("INF"))).1, "3");
    assert_eq!(shown(numeric_mod(&dbl("-0"), &dbl("2"))).1, "-0");
    assert_eq!(code(numeric_mod(&i("1"), &i("0"))), "FOAR0001");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §6.2.7,
/// §6.2.8] unary plus keeps a value, as its base numeric type; unary minus
/// negates it, zero of a decimal staying zero and a double's zero turning
/// negative.
#[test]
fn unary_operators() {
    assert_eq!(shown(numeric_unary_plus(&a(AtomicType::Short, "5"))), (AtomicType::Integer, "5".into()));
    assert_eq!(shown(numeric_unary_minus(&i("5"))).1, "-5");
    assert_eq!(shown(numeric_unary_minus(&dec("0.0"))).1, "0");
    assert_eq!(shown(numeric_unary_minus(&dbl("0"))).1, "-0");
    assert_eq!(shown(numeric_unary_minus(&dbl("-INF"))).1, "INF");
    assert_eq!(shown(numeric_unary_minus(&dbl("NaN"))).1, "NaN");
    assert_eq!(code(numeric_unary_minus(&a(AtomicType::Boolean, "true"))), "XPTY0004");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §6.3.1 to
/// §6.3.3] comparisons after promotion: zeros of either sign equal, NaN
/// equal to nothing and neither less nor greater; infinities beyond every
/// number.
#[test]
fn comparisons() {
    assert!(numeric_equal(&i("1"), &dbl("1")).unwrap());
    assert!(numeric_equal(&dec("1.50"), &dec("1.5")).unwrap());
    assert!(numeric_equal(&dbl("0"), &dbl("-0")).unwrap());
    assert!(!numeric_equal(&dbl("NaN"), &dbl("NaN")).unwrap());
    assert!(numeric_less_than(&i("-1"), &dec("0.5")).unwrap());
    assert!(!numeric_less_than(&dbl("NaN"), &dbl("1")).unwrap());
    assert!(numeric_greater_than(&dbl("INF"), &dbl("1e308")).unwrap());
    assert!(!numeric_greater_than(&dbl("1"), &dbl("NaN")).unwrap());
    assert!(numeric_less_than(&dbl("-INF"), &i("-99999999999999999999")).unwrap());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §6.2] an
/// integer or decimal result past `MAX_DIGITS` overflows, err:FOAR0002.
#[test]
fn decimal_overflow() {
    let long = "9".repeat(super::super::decimal::MAX_DIGITS);
    assert_eq!(code(numeric_add(&i(&long), &i("1"))), "FOAR0002");
    assert_eq!(code(numeric_multiply(&i(&long), &i("10"))), "FOAR0002");
    let too_long = format!("{long}9");
    assert_eq!(code(numeric_subtract(&i(&too_long), &i("1"))), "FOAR0002");
}
