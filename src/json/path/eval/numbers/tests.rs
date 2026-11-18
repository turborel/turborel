//! Tests of CEILING, FLOOR, ABS and MOD of the numbers an SQL/JSON item
//! may be.

use super::*;
use crate::json::path::eval::tests::{error, gives, state, value};

fn exact(units: i128, scale: u32) -> Number {
    Number::Exact(Decimal::new(units, scale))
}

fn decfloat(coefficient: i128, exponent: i64) -> Number {
    Number::DecFloat(DecFloat::new(coefficient, exponent, MAX_DECFLOAT_PRECISION).unwrap())
}

/// The coefficient and exponent of a decimal floating-point `v`.
fn digits(v: &Value) -> (i128, i32) {
    let Value::DecFloat(d) = v else { panic!("{v:?}") };
    (d.coefficient(), d.exponent())
}

/// The units and scale of an exact `v`.
fn units(v: &Value) -> (i128, u32) {
    let Value::Exact(d) = v else { panic!("{v:?}") };
    (d.units, d.scale)
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.31 GR15, GR16, SR20]
/// CEILING and FLOOR of an exact number are exact of scale 0: the least
/// whole number not below, the greatest not above; a whole number is
/// itself.
#[test]
fn ceiling_and_floor_of_exact_numbers() {
    assert_eq!(units(&ceiling(exact(12, 1)).unwrap()), (2_i128, 0_u32));
    assert_eq!(units(&ceiling(exact(-12, 1)).unwrap()), (-1_i128, 0_u32));
    assert_eq!(units(&floor(exact(12, 1)).unwrap()), (1_i128, 0_u32));
    assert_eq!(units(&floor(exact(-12, 1)).unwrap()), (-2_i128, 0_u32));
    assert_eq!(units(&floor(exact(30, 1)).unwrap()), (3_i128, 0_u32));
    assert_eq!(units(&ceiling(exact(7, 0)).unwrap()), (7_i128, 0_u32));
    assert_eq!(units(&ceiling(exact(1, 38)).unwrap()), (1_i128, 0_u32));
    assert_eq!(units(&floor(exact(-1, 38)).unwrap()), (-1_i128, 0_u32));
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.31 GR15b ii, GR16b ii,
/// SR20] CEILING and FLOOR of approximate and decimal floating-point
/// numbers keep their kind; one whole, or of a power of ten no i128 holds
/// below 1, is taken as a whole number.
#[test]
fn ceiling_and_floor_of_approximate_and_decfloat_numbers() {
    assert!(matches!(ceiling(Number::Approx(1.2_f64)).unwrap(), Value::Approx(x) if x.to_bits() == 2.0_f64.to_bits()));
    assert!(matches!(floor(Number::Approx(-1.2_f64)).unwrap(), Value::Approx(x) if x.to_bits() == (-2.0_f64).to_bits()));
    assert_eq!(digits(&ceiling(decfloat(12, -1)).unwrap()), (2_i128, 0_i32));
    assert_eq!(digits(&floor(decfloat(-12, -1)).unwrap()), (-2_i128, 0_i32));
    assert_eq!(digits(&floor(decfloat(5, 3)).unwrap()), (5_i128, 3_i32));
    assert_eq!(digits(&ceiling(decfloat(5, -50)).unwrap()), (1_i128, 0_i32));
    assert_eq!(digits(&floor(decfloat(5, -50)).unwrap()), (0_i128, 0_i32));
    assert_eq!(digits(&floor(decfloat(-5, -50)).unwrap()), (-1_i128, 0_i32));
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.31 GR8] ABS of each kind
/// of number: itself if not negative, else negated; of its kind and scale.
#[test]
fn abs_of_each_kind_of_number() {
    assert_eq!(units(&abs(exact(-15, 1))), (15_i128, 1_u32));
    assert_eq!(units(&abs(exact(15, 1))), (15_i128, 1_u32));
    assert!(matches!(abs(Number::Approx(-0.5_f64)), Value::Approx(x) if x.to_bits() == 0.5_f64.to_bits()));
    assert_eq!(digits(&abs(decfloat(-7, 2))), (7_i128, 2_i32));
    assert_eq!(digits(&abs(decfloat(7, 2))), (7_i128, 2_i32));
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.31 GR9c, SR12] MOD of
/// whole numbers: of the dividend's sign, below the divisor in magnitude;
/// exact of scale 0, or decimal floating-point if either operand is; a
/// whole exact number of a scale above 0 is taken. MC/DC of either
/// operand DECFLOAT: the dividend alone, the divisor alone, neither.
#[test]
fn modulo_of_whole_numbers() {
    assert_eq!(units(&modulo(exact(7, 0), exact(3, 0)).unwrap()), (1_i128, 0_u32));
    assert_eq!(units(&modulo(exact(-7, 0), exact(3, 0)).unwrap()), (-1_i128, 0_u32));
    assert_eq!(units(&modulo(exact(7, 0), exact(-3, 0)).unwrap()), (1_i128, 0_u32));
    assert_eq!(units(&modulo(exact(70, 1), exact(3, 0)).unwrap()), (1_i128, 0_u32));
    assert_eq!(digits(&modulo(decfloat(7, 0), exact(3, 0)).unwrap()), (1_i128, 0_i32));
    assert_eq!(digits(&modulo(exact(7, 0), decfloat(3, 0)).unwrap()), (1_i128, 0_i32));
    assert_eq!(digits(&modulo(decfloat(1, 3), decfloat(3, 0)).unwrap()), (1_i128, 0_i32));
    gives("lax $ % 4", "10", "[2]");
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.31 SR12, GR9b] MOD of an
/// approximate number, a number not whole, or one past an i128, is refused
/// with SQLCODE -301; of a zero divisor, SQLSTATE 22012.
#[test]
fn modulo_refuses_what_is_not_a_whole_exact_number() {
    for (n, m) in [
        (Number::Approx(7.0_f64), exact(3, 0)),
        (exact(7, 0), Number::Approx(3.0_f64)),
        (exact(75, 1), exact(3, 0)),
        (decfloat(75, -1), exact(3, 0)),
        (decfloat(1, 50), exact(3, 0)),
        (decfloat(1, -50), exact(3, 0)),
    ] {
        let e = modulo(n, m).unwrap_err();
        assert_eq!(e.sqlcode(), -301_i32, "{n:?} % {m:?}");
        assert_eq!(e.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §6.31 SR12"));
    }
    let e = modulo(exact(7, 0), exact(0, 2)).unwrap_err();
    assert_eq!(e.sqlstate().as_str(), "22012");
    assert_eq!(e.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §6.31 GR9b"));
    assert_eq!(state("lax 7.5 % 2", "0"), "22000");
    assert_eq!(error("lax 7 % 2e0", "0").sqlcode(), -301_i32);
    assert!(matches!(value("lax 7.0 % 2", "0"), Value::Exact(d) if d.units == 1));
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.31 GR15b i, GR16b i] CEILING of an
/// approximate number between -1 and 0, and FLOOR of -0, are 0 and not
/// -0, so the item serializes as 0. Regression: `ceiling()` gave -0.0
/// before the fix, and fails without it.
#[test]
fn ceiling_and_floor_never_give_negative_zero() {
    let zero = Some(0.0_f64.to_bits());
    assert_eq!(bits(&ceiling(Number::Approx(-0.5_f64)).unwrap()), zero);
    assert_eq!(bits(&floor(Number::Approx(-0.0_f64)).unwrap()), zero);
    assert_eq!(bits(&ceiling(Number::Approx(-0.0_f64)).unwrap()), zero);
    assert_eq!(bits(&floor(Number::Approx(-0.5_f64)).unwrap()), Some((-1.0_f64).to_bits()));
}

/// The bits of the approximate number `v`, so -0 and 0 differ; none of
/// another value.
fn bits(v: &Value) -> Option<u64> {
    let Value::Approx(x) = *v else { return None };
    Some(x.to_bits())
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR11i iii 2 C] [ISO/IEC
/// 9075-2:2023 §6.31 SR12] `%` of an approximate or a fractional operand
/// is the exception condition data exception, SQLSTATE 22000 explicit, so
/// lax mode's filters and ON ERROR take it; `5e0 % 2` is one.
#[test]
fn modulo_of_an_approximate_or_fractional_operand_is_22000() {
    for (n, m) in [(Number::Approx(5.0_f64), exact(2, 0)), (exact(55, 1), exact(2, 0))] {
        assert_eq!(modulo(n, m).unwrap_err().sqlstate(), SqlState::DATA_EXCEPTION);
    }
    assert_eq!(state("lax 5e0 % 2", "0"), "22000");
    gives("lax $ ? (@ % 2 == 0)", "[2, 2.5e0, 4]", "[2,4]");
}
