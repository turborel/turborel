//! Unit tests of the parts of set functions (`functions/partial.rs`).

use super::super::any_value::AnyValue;
use super::super::count::Count;
use super::super::totals::Sum;
use super::*;
use crate::value::lob::{LobKind, MAX_LOB_LENGTH};
use crate::value::{CharUnits, DOUBLE_PRECISION, Decimal, MAX_EXACT_PRECISION, Spelled};
use std::sync::Arc;

/// NUMERIC(18, 0).
const WHOLE: DataType = DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric };
/// DOUBLE PRECISION.
const DOUBLE: DataType = DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::Float };

fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// How `v` shows, to compare values by.
fn shown(v: &Value) -> String {
    format!("{v:?}")
}

/// The part of `func` of result `result` fed `values`.
fn part(func: &AggregateRef, result: &DataType, values: &[Value]) -> Value {
    let mut acc = Partial { func: func.clone(), result: result.clone() }.accumulator(PART_TYPE);
    for v in values {
        acc.update(v).unwrap();
    }
    acc.finish().unwrap()
}

/// Verifies: [DESIGN] RULES.md D341: a part of COUNT is its state, and
/// parts merged count every value; a part of a sum of approximate
/// numbers, which writes no state, is its result, which the merge gives;
/// of no part, the function's result of no value.
#[test]
fn parts_merge_by_state_or_give_their_result() {
    let count = AggregateRef(Arc::new(Count));
    let merged = Merged { func: count.clone(), result: WHOLE };
    let mut acc = merged.accumulator(WHOLE);
    assert_eq!(shown(&acc.finish().unwrap()), shown(&int(0)));
    acc.update(&part(&count, &WHOLE, &[int(1), int(2)])).unwrap();
    acc.update(&part(&count, &WHOLE, &[int(3)])).unwrap();
    assert_eq!(shown(&acc.finish().unwrap()), shown(&int(3)));
    let sum = AggregateRef(Arc::new(Sum));
    let mut whole = Merged { func: sum.clone(), result: DOUBLE }.accumulator(DOUBLE);
    assert_eq!(shown(&whole.finish().unwrap()), shown(&Value::Null));
    whole.update(&part(&sum, &DOUBLE, &[Value::Approx(0.5), Value::Approx(0.25)])).unwrap();
    assert_eq!(shown(&whole.finish().unwrap()), shown(&Value::Approx(0.75)));
}

/// Verifies: [DESIGN] RULES.md D341: a part is given a vector, integers,
/// rows and states as the function is; its state is the function's.
#[test]
fn a_part_is_fed_as_its_function() {
    let count = AggregateRef(Arc::new(Count));
    let mut acc = Partial { func: count.clone(), result: WHOLE }.accumulator(PART_TYPE);
    acc.update_many(&[int(1), Value::Null], None).unwrap();
    acc.update_ints(&Ints::new(vec![1, 2], 0, None), None, &crate::kernels::Portable).unwrap();
    acc.update_floats(&Floats::new(vec![1.0_f64], None), None).unwrap();
    acc.update_texts(&Texts::new("a".to_owned(), vec![1], None), None).unwrap();
    acc.update_rows(2).unwrap();
    let mut state = Vec::new();
    assert!(acc.write_state(&mut state));
    acc.merge(&state).unwrap();
    let mut merged = Merged { func: count, result: WHOLE }.accumulator(WHOLE);
    merged.update(&acc.finish().unwrap()).unwrap();
    assert_eq!(shown(&merged.finish().unwrap()), shown(&int(14)));
}

/// Verifies: [DESIGN] RULES.md D341: the merge's own state is a result
/// it holds or its function's state, taken in again as a part; robustness:
/// a value not a part, or a part of no kind, is refused (SQLCODE -916).
#[test]
fn a_merge_writes_what_it_takes_and_refuses_others() {
    let sum = AggregateRef(Arc::new(Sum));
    let mut held = Merged { func: sum.clone(), result: DOUBLE }.accumulator(DOUBLE);
    let mut state = Vec::new();
    assert!(!held.write_state(&mut state), "a sum of approximate numbers writes no state");
    held.update(&part(&sum, &DOUBLE, &[Value::Approx(2.0)])).unwrap();
    assert!(held.write_state(&mut state));
    let mut again = Merged { func: sum, result: DOUBLE }.accumulator(DOUBLE);
    again.merge(&state).unwrap();
    assert_eq!(shown(&again.finish().unwrap()), shown(&Value::Approx(2.0)));
    let count = AggregateRef(Arc::new(Count));
    let mut counted = Merged { func: count.clone(), result: WHOLE }.accumulator(WHOLE);
    counted.update(&part(&count, &WHOLE, &[int(1)])).unwrap();
    let mut written = Vec::new();
    assert!(counted.write_state(&mut written));
    counted.merge(&written).unwrap();
    assert_eq!(shown(&counted.finish().unwrap()), shown(&int(2)));
    assert_eq!(counted.update(&int(1)).unwrap_err().code, Code::Corrupt);
    assert_eq!(counted.merge(&[9]).unwrap_err().code, Code::Corrupt);
    assert_eq!(counted.merge(&[]).unwrap_err().code, Code::Corrupt);
}

/// Verifies: [DESIGN] RULES.md D341: a function writing its state is
/// split by it; one writing none by its result, unless that may be a
/// value a table keeps apart, which it is not split by at all. The parts'
/// functions name themselves and their types.
#[test]
fn functions_are_split_by_state_result_or_not() {
    let (count, sum, any) = (AggregateRef(Arc::new(Count)), AggregateRef(Arc::new(Sum)), AggregateRef(Arc::new(AnyValue)));
    assert_eq!(split_of(&count, &WHOLE), Some(Split::State));
    assert_eq!(split_of(&sum, &DOUBLE), Some(Split::Whole));
    let clob = DataType::Lob { kind: LobKind::Character, length: MAX_LOB_LENGTH, units: CharUnits::Characters };
    assert_eq!(split_of(&any, &clob), None);
    let partial = Partial { func: count.clone(), result: WHOLE };
    assert_eq!((partial.name(), partial.result_type(None).unwrap()), (PARTIAL, PART_TYPE));
    let merged = Merged { func: count, result: WHOLE };
    assert_eq!((merged.name(), merged.result_type(Some(PART_TYPE)).unwrap()), (MERGED, WHOLE));
}
