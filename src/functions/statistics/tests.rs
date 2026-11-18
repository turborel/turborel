//! Unit tests of the statistical set functions' accumulators
//! (`functions/statistics.rs`): each General Rule against values worked by
//! hand, of one value and none, states merged, and the result types.

use super::super::{AggregateFunction, ScalarFunction};
use super::{DECFLOAT, DOUBLE, Pair, Statistic};
use crate::error::{Code, Result, SqlState};
use crate::value::{DataType, DecFloat, Decimal, Spelled, Value};

/// The pairs (Y, X) worked by hand: (2, 1), (4, 2), (5, 3), (9, 4): N 4,
/// SX 10, SY 20, SXS 30, SYS 126, SXY 61; so SXX 5, SYY 26, SXY 11.
const PAIRS: [(i128, i128); 4] = [(2, 1), (4, 2), (5, 3), (9, 4)];

/// How near a double result must be to the value worked by hand.
const NEAR: f64 = 1e-12;

/// The pair of `y` and `x`, as `Pair` makes it.
fn pair(y: Value, x: Value) -> Value {
    Pair.call(&[y, x]).unwrap()
}

/// An exact number `n`.
fn exact(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// `kind` of the values `values`, of a result of type `result`.
fn finished(kind: Statistic, result: DataType, values: &[Value]) -> Result<Value> {
    let mut acc = kind.accumulator(result);
    for v in values {
        acc.update(v)?;
    }
    acc.finish()
}

/// `PAIRS`, each number made by `make`; of one argument, X alone.
fn worked(kind: Statistic, make: impl Fn(i128) -> Value) -> Vec<Value> {
    PAIRS.iter().map(|&(y, x)| if kind.binary() { pair(make(y), make(x)) } else { make(x) }).collect()
}

/// That `got` is a double near `want`.
fn near(got: &Value, want: f64, what: &str) {
    let Value::Approx(g) = got else { panic!("{what}: {got} is no double") };
    assert!((g - want).abs() <= NEAR * want.abs().max(1.0), "{what}: {g} is not {want}");
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 GR7d ix, GR8c, SR7i, SR7j] of
/// `PAIRS` in double precision, each set function's value worked by hand;
/// REGR_COUNT 4, exactly.
#[test]
fn each_of_values_worked_by_hand_10_9() {
    let r2 = 121.0_f64 / 130.0_f64;
    let want = [
        (Statistic::VarPop, 1.25_f64),
        (Statistic::VarSamp, 5.0_f64 / 3.0_f64),
        (Statistic::StddevPop, 1.25_f64.sqrt()),
        (Statistic::StddevSamp, (5.0_f64 / 3.0_f64).sqrt()),
        (Statistic::CovarPop, 2.75_f64),
        (Statistic::CovarSamp, 11.0_f64 / 3.0_f64),
        (Statistic::Corr, r2.sqrt()),
        (Statistic::Slope, 2.2_f64),
        (Statistic::Intercept, -0.5_f64),
        (Statistic::R2, r2),
        (Statistic::AvgX, 2.5_f64),
        (Statistic::AvgY, 5.0_f64),
        (Statistic::Sxx, 5.0_f64),
        (Statistic::Syy, 26.0_f64),
        (Statistic::Sxy, 11.0_f64),
    ];
    for (kind, w) in want {
        near(&finished(kind, DOUBLE, &worked(kind, exact)).unwrap(), w, kind.name());
    }
    let counted = Statistic::Count.result_type(Some(Pair.return_type(&[DOUBLE, DOUBLE]).unwrap())).unwrap();
    assert_eq!(finished(Statistic::Count, counted, &worked(Statistic::Count, exact)).unwrap().to_string(), "4");
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 GR7d ii, GR7d ix 2 A, GR8c i,
/// GR8c ii, GR8c ix 1, GR8c xii 1] of no values REGR_COUNT is 0 and the
/// others null; of one value VAR_POP and REGR_SXX 0, VAR_SAMP,
/// STDDEV_SAMP, COVAR_SAMP, REGR_SLOPE and CORR null.
#[test]
fn of_one_value_and_none_10_9() {
    for kind in Statistic::EVERY {
        let none = finished(kind, DOUBLE, &[]).unwrap();
        assert_eq!(none.to_string(), if kind == Statistic::Count { "0" } else { "NULL" }, "{kind:?}");
    }
    let one = |kind: Statistic| {
        let v = if kind.binary() { pair(Value::Approx(7.0), Value::Approx(3.0)) } else { Value::Approx(3.0) };
        finished(kind, DOUBLE, &[v]).unwrap().to_string()
    };
    let want = [
        (Statistic::VarPop, "0E0"),
        (Statistic::VarSamp, "NULL"),
        (Statistic::StddevSamp, "NULL"),
        (Statistic::CovarSamp, "NULL"),
        (Statistic::Sxx, "0E0"),
        (Statistic::AvgY, "7E0"),
        (Statistic::Slope, "NULL"),
        (Statistic::Corr, "NULL"),
    ];
    for (kind, w) in want {
        assert_eq!(one(kind), w, "{kind:?}");
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 GR8a] the pair is null if Y or X
/// is, so it is eliminated, else the row of the two.
#[test]
fn pair_of_a_null_is_null_10_9() {
    let one = Value::Approx(1.0);
    assert!(pair(Value::Null, one.clone()).is_null());
    assert!(pair(one.clone(), Value::Null).is_null());
    assert!(matches!(pair(one.clone(), one), Value::Row(_)));
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 GR8c x, xi, xiii] MC/DC of
/// REGR_R2 and CORR, of doubles and of DECFLOATs: X alike decides null
/// (CORR and REGR_R2); X spread and Y alike decide CORR null and REGR_R2
/// 1, REGR_INTERCEPT Y's value; both spread, the ratio, 1 of a line.
#[test]
fn values_alike_10_9() {
    let fit = |(y, x): ([i128; 2], [i128; 2]), kind: Statistic, result: DataType| {
        let values: Vec<Value> = y.iter().zip(&x).map(|(&y, &x)| pair(exact(y), exact(x))).collect();
        let got = finished(kind, result, &values).unwrap().to_string();
        got.trim_end_matches("E0").to_owned()
    };
    let (x_alike, y_alike, line) = (([1, 5], [2, 2]), ([6, 6], [1, 3]), ([1, 3], [1, 3]));
    for result in [DOUBLE, DECFLOAT] {
        assert_eq!(fit(x_alike, Statistic::R2, result.clone()), "NULL");
        assert_eq!(fit(x_alike, Statistic::Corr, result.clone()), "NULL");
        assert_eq!(fit(y_alike, Statistic::Corr, result.clone()), "NULL");
        assert_eq!(fit(y_alike, Statistic::R2, result.clone()), "1");
        assert_eq!(fit(y_alike, Statistic::Intercept, result.clone()), "6");
        assert_eq!(fit(line, Statistic::R2, result.clone()), "1");
        assert_eq!(fit(line, Statistic::Corr, result), "1");
    }
}

/// Verifies: [DESIGN] RULES.md D53: the states of two parts of `PAIRS`,
/// split at each place and merged, give what all of them do; a state of
/// no values merged changes nothing; a short state is SQLCODE -916.
#[test]
fn states_merge_10_9() {
    let values = worked(Statistic::Slope, exact);
    for kind in [Statistic::Slope, Statistic::CovarSamp, Statistic::Corr, Statistic::Syy] {
        let Value::Approx(whole) = finished(kind, DOUBLE, &values).unwrap() else { panic!("{kind:?} is a double") };
        for split in 0..=values.len() {
            let (head, tail) = values.split_at(split);
            let (mut a, mut b) = (kind.accumulator(DOUBLE), kind.accumulator(DOUBLE));
            for v in head {
                a.update(v).unwrap();
            }
            for v in tail {
                b.update(v).unwrap();
            }
            let mut state = Vec::new();
            assert!(b.write_state(&mut state));
            a.merge(&state).unwrap();
            near(&a.finish().unwrap(), whole, &format!("{kind:?} split at {split}"));
        }
    }
    let mut acc = Statistic::VarPop.accumulator(DOUBLE);
    assert_eq!(acc.merge(&[0; 3]).unwrap_err().code, Code::Corrupt);
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 GR3e, GR8c xii 2, SR7i] a sum of
/// squares past a double's range raises 22003, as a DECFLOAT square past
/// its range; DECFLOAT's accumulator writes no state; of DECFLOAT values
/// REGR_SLOPE is 11 / 5 exactly and STDDEV_POP √1.25 to 34 digits.
#[test]
fn out_of_range_and_decimal_10_9() {
    let huge = [Value::Approx(1e300), Value::Approx(-1e300)];
    assert_eq!(finished(Statistic::VarPop, DOUBLE, &huge).unwrap_err().state, Some(SqlState::NUMERIC_VALUE_OUT_OF_RANGE));
    let mut squared = Statistic::VarPop.accumulator(DECFLOAT);
    let past = squared.update(&Value::DecFloat(DecFloat::new(9, 4000, 34).unwrap())).unwrap_err();
    assert_eq!(past.state, Some(SqlState::NUMERIC_VALUE_OUT_OF_RANGE));
    let decimal = |n: i128| Value::DecFloat(DecFloat::new(n, 0, 34).unwrap());
    assert!(!Statistic::Slope.accumulator(DECFLOAT).write_state(&mut Vec::new()));
    assert_eq!(finished(Statistic::Slope, DECFLOAT, &worked(Statistic::Slope, decimal)).unwrap().to_string(), "2.2");
    let root = finished(Statistic::StddevPop, DECFLOAT, &worked(Statistic::StddevPop, decimal)).unwrap();
    assert_eq!(root.to_string(), "1.118033988749894848204586834365638");
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 SR7h, SR9c] VAR_POP of a number
/// DOUBLE PRECISION, of a DECFLOAT DECFLOAT(34), of a truth value -103;
/// REGR_COUNT exact of scale 0, the others DECFLOAT if Y or X is; the
/// pair's operands numbers, else -103.
#[test]
fn result_types_10_9() {
    let int = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer };
    assert_eq!(Statistic::VarPop.result_type(Some(int.clone())).unwrap(), DOUBLE);
    assert_eq!(Statistic::VarPop.result_type(Some(DECFLOAT)).unwrap(), DECFLOAT);
    assert_eq!(Statistic::VarSamp.result_type(Some(DataType::Boolean)).unwrap_err().code, Code::SyntaxRule);
    let mixed = Pair.return_type(&[DECFLOAT, int.clone()]).unwrap();
    assert_eq!(Statistic::Slope.result_type(Some(mixed.clone())).unwrap(), DECFLOAT);
    assert!(matches!(Statistic::Count.result_type(Some(mixed)).unwrap(), DataType::Exact { scale: 0, .. }));
    assert_eq!(Pair.return_type(&[int, DataType::Boolean]).unwrap_err().code, Code::SyntaxRule);
}
