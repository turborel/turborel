//! Unit tests of scaled set functions (`functions/partial/scaled.rs`).

use super::super::super::count::Count;
use super::super::super::totals::{Avg, Sum};
use super::*;
use crate::value::{MAX_EXACT_PRECISION, Spelled};
use std::sync::Arc;

/// NUMERIC(38, 2).
const CENTS: DataType = DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 2, spelled: Spelled::Numeric };

/// Half the largest 128-bit integer, 2^126 - 1.
const HALF: i128 = 85_070_591_730_234_615_865_843_651_857_942_052_863;

fn cents(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 2))
}

fn rows(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// The pair of `v` and `n` rows, as `Paired` makes it.
fn pair(v: Value, n: i128) -> Value {
    Paired(DataType::Boolean).call(&[v, rows(n)]).unwrap()
}

/// The result of `func` as `kind`, scaled, fed `pairs`.
fn scaled(func: AggregateRef, kind: ColumnarKind, pairs: &[(Value, i128)]) -> Result<Value> {
    let mut acc = Scaled { func, kind, result: CENTS }.accumulator(CENTS);
    for (v, n) in pairs {
        acc.update(&pair(v.clone(), *n))?;
    }
    acc.finish()
}

/// The result of `func` fed each value as many times as its count.
fn repeated(func: &AggregateRef, pairs: &[(Value, i128)]) -> Value {
    let mut acc = func.0.accumulator(CENTS);
    for (v, n) in pairs {
        for _ in 0..*n {
            acc.update(v).unwrap();
        }
    }
    acc.finish().unwrap()
}

/// Verifies: [DESIGN] RULES.md D662: COUNT, SUM and AVG of values each
/// of n rows are the functions of each value n times; of none, the
/// functions' own results of no value.
#[test]
fn a_value_of_n_rows_is_n_values() {
    let pairs = [(cents(150), 3), (cents(-25), 2), (cents(7), 1)];
    for (func, kind) in [
        (AggregateRef(Arc::new(Count)), ColumnarKind::Count),
        (AggregateRef(Arc::new(Sum)), ColumnarKind::Sum),
        (AggregateRef(Arc::new(Avg)), ColumnarKind::Avg),
    ] {
        let got = scaled(func.clone(), kind, &pairs).unwrap();
        assert_eq!(format!("{got:?}"), format!("{:?}", repeated(&func, &pairs)));
        let none = scaled(func.clone(), kind, &[]).unwrap();
        assert_eq!(format!("{none:?}"), format!("{:?}", repeated(&func, &[])));
    }
}

/// Verifies: [DESIGN] RULES.md D662: a value times its rows past 128 bits
/// raises SQLCODE -900, as a sum on the way does; the pair of a null is
/// null, so it is taken out as a null of every row.
#[test]
fn a_product_past_128_bits_raises_and_a_null_pairs_as_null() {
    let sum = AggregateRef(Arc::new(Sum));
    let err = scaled(sum.clone(), ColumnarKind::Sum, &[(cents(HALF), 3)]).unwrap_err();
    assert_eq!(err.code, Code::Unsupported);
    // Within 128 bits it is taken, and judged once as the sum's result.
    let judged = scaled(sum, ColumnarKind::Sum, &[(cents(HALF), 2)]).err().map(|e| e.code);
    assert_ne!(judged, Some(Code::Unsupported));
    assert!(pair(Value::Null, 4).is_null());
}

/// Verifies: [DESIGN] RULES.md D662: a scaled function's state is its
/// function's, so parts of it merge as the function's do.
#[test]
fn a_scaled_state_is_the_functions() {
    let sum = AggregateRef(Arc::new(Sum));
    let mut acc = Scaled { func: sum.clone(), kind: ColumnarKind::Sum, result: CENTS }.accumulator(CENTS);
    acc.update(&pair(cents(10), 4)).unwrap();
    let mut state = Vec::new();
    assert!(acc.write_state(&mut state));
    let mut other = sum.0.accumulator(CENTS);
    other.merge(&state).unwrap();
    assert_eq!(format!("{:?}", other.finish().unwrap()), format!("{:?}", cents(40)));
}
