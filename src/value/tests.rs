use super::*;

/// Verifies: [X3.135-1986 §5.9 GR5] an approximate zero divisor is a data exception.
///
/// Decision `op == Div && y == 0.0`. MC/DC: (T T) error; (F T) adding zero is
/// fine; (T F) dividing by one is fine.
#[test]
fn approximate_division_by_zero() {
    let one = Value::Approx(1.0);
    let zero = Value::Approx(0.0);
    assert_eq!(arith(ArithOp::Div, &one, &zero).map_err(|e| e.code).err(), Some(Code::Data)); // T T
    assert!(arith(ArithOp::Add, &one, &zero).is_ok()); // F T
    assert!(arith(ArithOp::Div, &one, &one).is_ok()); // T F
}

/// Verifies: [X3.135-1986 §5.9 SR3, §8.7 SR6] [ISO/IEC 9075:1992 §9.2 SR2] a value of the wrong type,
/// as a plug-in function might return, is a data exception in arithmetic
/// and in assignment, not a panic.
#[test]
fn mistyped_values_are_reported() {
    let text = Value::Char("A".into());
    let one = Value::Exact(Decimal::new(1, 0));
    assert_eq!(arith(ArithOp::Add, &text, &one).unwrap_err().code, Code::Data);
    let approximate = DataType::Approximate { precision: REAL_PRECISION, spelled: Spelled::Float };
    assert_eq!(assign(text.clone(), &approximate, "test").unwrap_err().code, Code::Data);
    let exact = DataType::Exact { precision: 5, scale: 0, spelled: Spelled::Numeric };
    assert_eq!(assign(text, &exact, "test").unwrap_err().code, Code::Data);
    let tiny_scale = DataType::Exact { precision: 5, scale: 0, spelled: Spelled::Numeric };
    assert_eq!(assign(Value::Exact(Decimal::new(1, u32::MAX)), &tiny_scale, "test").unwrap_err().code, Code::Data);
}

/// Verifies: [DESIGN] an assignment's error cites ISO/IEC 9075:1992 once
/// when the rule that assigns is of it too, else both standards.
#[test]
fn assignment_rules_cite_each_standard_once() {
    assert_eq!(assignment_rule("ISO/IEC 9075:1992 §13.8 GR4b iii", "§9.2 GR3b ii"), "ISO/IEC 9075:1992 §13.8 GR4b iii, §9.2 GR3b ii");
    assert_eq!(assignment_rule("X3.135-1986 §8.7 GR1", "§9.2 GR3b ii"), "X3.135-1986 §8.7 GR1; ISO/IEC 9075:1992 §9.2 GR3b ii");
}
