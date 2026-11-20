//! Unit tests of the functions of arrays (`arrays.rs`): the types they
//! report, and the maximum cardinality they keep.

use std::sync::Arc;

use super::*;
use crate::error::SqlState;

fn int() -> DataType {
    DataType::Exact { precision: 9, scale: 0, spelled: crate::value::Spelled::Integer }
}

fn ints(n: usize) -> Value {
    Value::Array((0..n).map(|i| Value::Exact(Decimal::new(i128::try_from(i).unwrap(), 0))).collect())
}

/// Verifies: [DESIGN] RULES.md D68, each function of arrays, bound as its
/// syntax is read, reports the name its calls show and the type its
/// binding gave it, whatever its arguments' types.
#[test]
fn each_function_reports_its_bound_type() {
    let array = DataType::array(int(), 10);
    let functions: [(&dyn ScalarFunction, &str, &DataType); 6] = [
        (&ArrayOf(array.clone()), ARRAY, &array),
        (&ElementOf(int()), ELEMENT, &int()),
        (&CardinalityOf(int()), CARDINALITY, &int()),
        (&TrimArrayOf(array.clone()), TRIM_ARRAY, &array),
        (&ArrayConcatenation(array.clone()), ARRAY_CONCATENATION, &array),
        (&AssignElement(array.clone(), 10), ASSIGN_ELEMENT, &array),
    ];
    for (f, name, t) in functions {
        assert_eq!(f.name(), name);
        assert_eq!(&f.return_type(&[]).unwrap(), t, "{name}");
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.47 GR2b] an array concatenation of
/// more elements than the maximum cardinality is an exception condition,
/// data exception, array data, right truncation; of one null operand it
/// is the null value; of exactly the maximum it is the array.
#[test]
fn a_concatenation_past_the_maximum_cardinality_is_truncation() {
    let f = ArrayConcatenation(DataType::array(int(), MAX_ARRAY_CARDINALITY));
    let half = most().div_euclid(2);
    let e = f.call(&[ints(half), ints(half.saturating_add(2))]).unwrap_err();
    assert_eq!(e.sqlstate(), SqlState::ARRAY_DATA_RIGHT_TRUNCATION, "{e}");
    assert!(e.to_string().contains("§6.47 GR2b"), "{e}");
    let Value::Array(whole) = f.call(&[ints(half), ints(most().saturating_sub(half))]).unwrap() else { panic!("an array") };
    assert_eq!(whole.len(), most());
    assert!(f.call(&[Value::Null, ints(1)]).unwrap().is_null());
}

/// The row of an ARRAY_AGG's value `n` and its sort key.
fn keyed(n: usize) -> Value {
    let v = Value::Exact(Decimal::new(i128::try_from(n).unwrap(), 0));
    Value::Row(Arc::from(vec![v.clone(), v]))
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 GR12e] ARRAY_AGG of more rows than
/// the maximum cardinality raises array data, right truncation as the row
/// past it is taken in, citing §6.49 GR3b of an array value constructor by
/// query; of fewer, the values in order of their keys.
#[test]
fn an_aggregate_past_the_maximum_cardinality_is_truncation() {
    for (by_query, rule) in [(false, "§10.9 GR12e"), (true, "§6.49 GR3b")] {
        let f = ArrayAggregate { orders: vec![SortOrder { descending: true, nulls_first: false }], collations: vec![None], by_query };
        let mut acc = f.accumulator(DataType::array(int(), MAX_ARRAY_CARDINALITY));
        for n in 0..most() {
            acc.update(&keyed(n)).unwrap();
        }
        let e = acc.update(&keyed(0)).unwrap_err();
        assert_eq!(e.sqlstate(), SqlState::ARRAY_DATA_RIGHT_TRUNCATION, "{e}");
        assert!(e.to_string().contains(rule), "{e}");
        let Value::Array(all) = acc.finish().unwrap() else { panic!("an array") };
        assert_eq!((all.len(), all[0].to_string()), (most(), (most() - 1).to_string()), "descending");
    }
}

/// Verifies: [DESIGN] RULES.md D68, ARRAY_AGG is bound with a row of its
/// value and sort keys: given anything else, as type or as value, it
/// refuses with SQLCODE -910, a fault of the binding, not of the data.
#[test]
fn an_aggregate_refuses_a_value_not_its_row() {
    let f = ArrayAggregate { orders: Vec::new(), collations: Vec::new(), by_query: false };
    assert_eq!(f.result_type(Some(int())).unwrap_err().sqlcode(), -910_i32);
    assert_eq!(f.result_type(None).unwrap_err().sqlcode(), -910_i32);
    let mut acc = f.accumulator(DataType::array(int(), 10));
    assert_eq!(acc.update(&Value::Exact(Decimal::new(1, 0))).unwrap_err().sqlcode(), -910_i32);
    assert!(acc.finish().unwrap().is_null(), "of no rows, null");
}
