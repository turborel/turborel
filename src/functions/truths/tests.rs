//! Unit tests of EVERY's and ANY's accumulators (`functions/truths.rs`).

use super::*;
use crate::error::Code;

/// The result of `f` given `values` one at a time, as text.
fn of(f: &Truths, values: &[bool]) -> String {
    let mut acc = f.accumulator(DataType::Boolean);
    for &b in values {
        acc.update(&Value::Bool(b)).unwrap();
    }
    acc.finish().unwrap().to_string()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR7d ii, vii, viii] EVERY is
/// False once a value is False, ANY True once a value is True, each null
/// of no values.
#[test]
fn every_and_any_10_9() {
    assert_eq!(of(&Truths::EVERY, &[]), "NULL");
    assert_eq!(of(&Truths::EVERY, &[true, true]), "TRUE");
    assert_eq!(of(&Truths::EVERY, &[true, false, true]), "FALSE");
    assert_eq!(of(&Truths::ANY, &[]), "NULL");
    assert_eq!(of(&Truths::ANY, &[false, false]), "FALSE");
    assert_eq!(of(&Truths::SOME, &[false, true, false]), "TRUE");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 SR7e] the argument is a BOOLEAN,
/// and so is the result; any other is SQLCODE -103.
#[test]
fn the_argument_is_boolean_10_9() {
    assert_eq!(Truths::EVERY.result_type(Some(DataType::Boolean)).unwrap(), DataType::Boolean);
    assert_eq!(Truths::ANY.result_type(Some(DataType::Date)).unwrap_err().code, Code::SyntaxRule);
    assert_eq!(Truths::SOME.result_type(None).unwrap_err().code, Code::SyntaxRule);
    assert_eq!((Truths::EVERY.name(), Truths::ANY.name(), Truths::SOME.name()), ("EVERY", "ANY", "SOME"));
}

/// Verifies: [DESIGN] RULES.md D38: a state written and merged gives the
/// result of the values given at once, a state of none changing nothing;
/// a value not a truth value is SQLCODE -910.
#[test]
fn states_merge_as_values_d38() {
    for (f, first, then) in [(&Truths::EVERY, true, false), (&Truths::ANY, false, true)] {
        let mut a = f.accumulator(DataType::Boolean);
        a.update(&Value::Bool(first)).unwrap();
        let mut b = f.accumulator(DataType::Boolean);
        let mut none = Vec::new();
        assert!(b.write_state(&mut none));
        b.update(&Value::Bool(then)).unwrap();
        let mut state = Vec::new();
        assert!(a.write_state(&mut state));
        b.merge(&state).unwrap();
        b.merge(&none).unwrap();
        assert_eq!(b.finish().unwrap().to_string(), Value::Bool(then).to_string());
    }
    let mut a = Truths::EVERY.accumulator(DataType::Boolean);
    assert_eq!(a.update(&Value::Char("TRUE".into())).unwrap_err().code, Code::Api);
}
