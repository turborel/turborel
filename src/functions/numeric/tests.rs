//! Unit tests of the numeric value functions (`functions/numeric.rs`):
//! each General Rule of §6.31 GR12 to GR16, of each type of operand.

use super::super::ScalarFunction;
use super::{DECFLOAT, DOUBLE, Numeric, square_root};
use crate::error::{Code, SqlState};
use crate::value::{DataType, DecFloat, Decimal, Spelled, Value};

/// An exact number `units` × 10^−`scale`.
fn exact(units: i128, scale: u32) -> Value {
    Value::Exact(Decimal::new(units, scale))
}

/// A DECFLOAT `c` × 10^`e`.
fn decimal(c: i128, e: i64) -> Value {
    Value::DecFloat(DecFloat::new(c, e, 34).unwrap())
}

/// `f` of `args`, shown.
fn shown(f: Numeric, args: &[Value]) -> String {
    f.call(args).unwrap().to_string()
}

/// The SQLSTATE `f` of `args` fails with.
fn state(f: Numeric, args: &[Value]) -> Option<SqlState> {
    f.call(args).unwrap_err().state
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.31 GR14] MC/DC of POWER's cases:
/// base 0 and exponent below 0 (2201F), 0 (1) or above (0); a negative
/// base and an exponent not whole (2201F), whole and even (positive),
/// whole and odd (negative), of exact, double and DECFLOAT exponents; a
/// positive base.
#[test]
fn power_cases_6_31() {
    let invalid = Some(SqlState::INVALID_ARGUMENT_FOR_POWER_FUNCTION);
    assert_eq!(state(Numeric::Power, &[exact(0, 0), exact(-1, 0)]), invalid);
    assert_eq!(shown(Numeric::Power, &[exact(0, 0), exact(0, 0)]), "1E0");
    assert_eq!(shown(Numeric::Power, &[Value::Approx(0.0), exact(3, 0)]), "0E0");
    assert_eq!(state(Numeric::Power, &[exact(-8, 0), exact(5, 1)]), invalid);
    assert_eq!(state(Numeric::Power, &[exact(-8, 0), Value::Approx(0.5)]), invalid);
    assert_eq!(state(Numeric::Power, &[exact(-8, 0), decimal(5, -1)]), invalid);
    assert_eq!(shown(Numeric::Power, &[exact(-2, 0), exact(20, 1)]), "4E0");
    assert_eq!(shown(Numeric::Power, &[exact(-2, 0), Value::Approx(3.0)]), "-8E0");
    assert_eq!(shown(Numeric::Power, &[decimal(-2, 0), decimal(3, 0)]), "-8");
    assert_eq!(shown(Numeric::Power, &[decimal(-2, 0), decimal(1, 1)]), "1024");
    assert_eq!(shown(Numeric::Power, &[exact(2, 0), exact(-1, 0)]), "5E-1");
    assert_eq!(state(Numeric::Power, &[exact(10, 0), exact(400, 0)]), Some(SqlState::NUMERIC_VALUE_OUT_OF_RANGE));
    assert_eq!(shown(Numeric::Power, &[decimal(10, 0), exact(400, 0)]), "1E400");
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.31 GR12, GR13] LN of 0 or below
/// 2201E, of e^2 2; of a DECFLOAT past a double's range, through its
/// exponent; EXP of 0 1, past the range 22003, far below it 0.
#[test]
fn logarithm_and_exponential_6_31() {
    let invalid = Some(SqlState::INVALID_ARGUMENT_FOR_NATURAL_LOGARITHM);
    assert_eq!(state(Numeric::Ln, &[exact(0, 0)]), invalid);
    assert_eq!(state(Numeric::Ln, &[decimal(-1, 0)]), invalid);
    assert_eq!(shown(Numeric::Ln, &[Value::Approx(1.0)]), "0E0");
    assert_eq!(shown(Numeric::Ln, &[decimal(1, 400)]), "921.0340371976183");
    assert_eq!(shown(Numeric::Exp, &[exact(0, 0)]), "1E0");
    assert_eq!(state(Numeric::Exp, &[Value::Approx(1000.0)]), Some(SqlState::NUMERIC_VALUE_OUT_OF_RANGE));
    assert_eq!(state(Numeric::Exp, &[decimal(1, 5)]), Some(SqlState::NUMERIC_VALUE_OUT_OF_RANGE));
    assert_eq!(shown(Numeric::Exp, &[decimal(-1, 5)]), "0");
    assert_eq!(shown(Numeric::Exp, &[decimal(2, 0)]).get(..10), Some("7.38905609"));
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.31 SR19] SQRT of a DECFLOAT to its
/// 34 digits: of 2, of 1E-401, of 4 exactly 2; of 0, 0; none of a
/// negative.
#[test]
fn square_roots_6_31() {
    let root = |c: i128, e: i64| square_root(DecFloat::new(c, e, 34).unwrap()).map(|d| d.to_string());
    assert_eq!(root(2, 0).as_deref(), Some("1.414213562373095048801688724209698"));
    assert_eq!(root(4, 0).as_deref(), Some("2"));
    assert_eq!(root(1, -400).as_deref(), Some("1E-200"));
    assert_eq!(root(0, 0).as_deref(), Some("0"));
    assert_eq!(root(-4, 0), None);
    assert_eq!(state(Numeric::Sqrt, &[exact(-1, 0)]), Some(SqlState::INVALID_ARGUMENT_FOR_POWER_FUNCTION));
    assert_eq!(shown(Numeric::Sqrt, &[exact(16, 0)]), "4E0");
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.31 GR15, GR16] FLOOR and CEILING of
/// exact numbers, negative and positive, whole and not; of doubles; of
/// DECFLOATs, whole, of digits below the point, and all below it.
#[test]
fn floor_and_ceiling_6_31() {
    let both = |v: Value| (shown(Numeric::Floor, std::slice::from_ref(&v)), shown(Numeric::Ceiling, &[v]));
    let pair = |f: &str, c: &str| (f.to_owned(), c.to_owned());
    assert_eq!(both(exact(-25, 1)), pair("-3", "-2"));
    assert_eq!(both(exact(25, 1)), pair("2", "3"));
    assert_eq!(both(exact(7, 0)), pair("7", "7"));
    assert_eq!(both(exact(-700, 2)), pair("-7", "-7"));
    assert_eq!(both(Value::Approx(-2.5)), pair("-3E0", "-2E0"));
    assert_eq!(both(decimal(-25, -1)), pair("-3", "-2"));
    assert_eq!(both(decimal(5, 3)), pair("5000", "5000"));
    assert_eq!(both(decimal(1, -50)), pair("0", "1"));
    assert_eq!(both(decimal(-1, -50)), pair("-1", "0"));
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.31 SR16 to SR20] the result types:
/// of FLOOR of NUMERIC(5, 2) NUMERIC(4, 0), of INTEGER INTEGER, of a
/// double or DECFLOAT its type; of LN a double, of a DECFLOAT DECFLOAT;
/// of POWER DECFLOAT if either operand is; an operand not a number -103;
/// a null operand null.
#[test]
fn result_types_6_31() {
    let numeric = DataType::Exact { precision: 5, scale: 2, spelled: Spelled::Decimal };
    let int = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };
    assert_eq!(Numeric::Floor.return_type(&[numeric]).unwrap(), DataType::Exact { precision: 4, scale: 0, spelled: Spelled::Numeric });
    assert_eq!(Numeric::Ceiling.return_type(std::slice::from_ref(&int)).unwrap(), int);
    assert_eq!(Numeric::Floor.return_type(&[DOUBLE]).unwrap(), DOUBLE);
    assert_eq!(Numeric::Ln.return_type(std::slice::from_ref(&int)).unwrap(), DOUBLE);
    assert_eq!(Numeric::Ln.return_type(&[DECFLOAT]).unwrap(), DECFLOAT);
    assert_eq!(Numeric::Power.return_type(&[int.clone(), DECFLOAT]).unwrap(), DECFLOAT);
    assert_eq!(Numeric::Exp.return_type(&[DataType::Boolean]).unwrap_err().code, Code::SyntaxRule);
    assert_eq!(Numeric::named("CEIL"), Some(Numeric::Ceiling));
    assert!(Numeric::EVERY.iter().all(|f| f.call(&vec![Value::Null; f.operands()]).unwrap().is_null()));
}
