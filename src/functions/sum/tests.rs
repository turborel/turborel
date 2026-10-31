//! Unit tests of SUM's and AVG's accumulators (`functions/sum.rs`).

use super::super::{AggregateFunction, Avg, Sum};
use super::*;
use crate::error::Code;
use crate::value::interval::Qualifier;
use crate::value::{DOUBLE_PRECISION, MAX_EXACT_PRECISION, Spelled, datetime::Field, pow10};

/// NUMERIC(18, 0), the type of SUM of an integer column.
const WHOLE: DataType = DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric };
/// DOUBLE PRECISION.
const DOUBLE: DataType = DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::Float };

/// The orders of three values.
const ORDERS: [[usize; 3]; 6] = [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]];

/// The bits of the approximate number `acc` finishes with; none else.
fn bits(acc: &dyn Accumulator) -> Option<u64> {
    if let Ok(Value::Approx(f)) = acc.finish() { Some(f.to_bits()) } else { None }
}

/// What `function`'s accumulator of `result` finishes with, fed `values`
/// in `order`: a value at a time, a vector at a time, and as integers
/// where all are exact of scale 0.
fn finished(function: &dyn AggregateFunction, result: DataType, values: &[Value], order: [usize; 3]) -> Vec<String> {
    let ordered: Vec<Value> = order.iter().filter_map(|&n| values.get(n).cloned()).collect();
    let mut one = function.accumulator(result);
    let mut ways = vec![ordered.iter().try_for_each(|v| one.update(v)).and_then(|()| one.finish())];
    let mut many = function.accumulator(result);
    ways.push(many.update_many(&ordered, None).and_then(|()| many.finish()));
    let units: Option<Vec<i64>> = ordered
        .iter()
        .map(|v| match v {
            Value::Exact(d) => i64::try_from(d.units).ok().filter(|_| d.scale == 0),
            Value::Null
            | Value::Approx(_)
            | Value::Char(_)
            | Value::Bits(_)
            | Value::Datetime(_)
            | Value::Interval(_)
            | Value::Extension(_) => None,
        })
        .collect();
    if let Some(units) = units {
        let mut ints = function.accumulator(result);
        ways.push(ints.update_ints(&Ints::new(units, 0, None), None).and_then(|()| ints.finish()));
    }
    let floats: Option<Vec<f64>> = ordered.iter().map(|v| if let Value::Approx(f) = v { Some(*f) } else { None }).collect();
    if let Some(floats) = floats {
        let mut many_floats = function.accumulator(result);
        ways.push(many_floats.update_floats(&Floats::new(floats, None), None).and_then(|()| many_floats.finish()));
    }
    ways.iter().map(|r| r.as_ref().map(ToString::to_string).map_err(|e| e.sqlstate().to_string())).map(|r| format!("{r:?}")).collect()
}

/// Verifies: [DESIGN] RULES.md D15 [ISO/IEC 9075:1992 §6.5 GR2b iv]: SUM
/// of 10^18 - 1, 1 and -1, whose total fits 18 digits, gives it in every
/// order, a value, a vector or integers at a time, as does AVG its mean;
/// a total past 18 digits is 22003 in every order. Regression test
/// (R-TEST-7): a running sum was judged, so 10^18 - 1 then 1 raised.
#[test]
fn an_exact_sum_is_judged_as_a_total() {
    let most = pow10(MAX_EXACT_PRECISION).unwrap() - 1_i128;
    let exact = |u: i128| Value::Exact(Decimal::new(u, 0));
    let fits = [exact(most), exact(1_i128), exact(-1_i128)];
    let past = [exact(most), exact(1_i128), exact(0_i128)];
    let mean = DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 2, spelled: Spelled::Numeric };
    for order in ORDERS {
        for got in finished(&Sum, WHOLE, &fits, order) {
            assert_eq!(got, "Ok(\"999999999999999999\")", "SUM in {order:?}");
        }
        for got in finished(&Avg, mean, &[exact(10_i128), exact(1_i128), exact(-1_i128)], order) {
            assert_eq!(got, "Ok(\"3.33\")", "AVG in {order:?}");
        }
        for got in finished(&Sum, WHOLE, &past, order) {
            assert_eq!(got, "Err(\"22003\")", "SUM past 18 digits in {order:?}");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D32 [ISO/IEC 9075:1992 §6.5 GR2b ii]: AVG judges
/// its mean, not its sum, which SQL-92 sets no range for: the mean of
/// 10^18 - 1 twice is 10^18 - 1, AVG of a scale that holds it.
#[test]
fn an_average_is_judged_alone() {
    let most = Value::Exact(Decimal::new(pow10(MAX_EXACT_PRECISION).unwrap() - 1_i128, 0));
    let mut avg = Avg.accumulator(WHOLE);
    avg.update_many(&[most.clone(), most], None).unwrap();
    assert_eq!(avg.finish().unwrap().to_string(), "999999999999999999");
}

/// Verifies: [DESIGN] RULES.md D18 (robustness, R-TEST-2): a sum 128 bits
/// cannot hold, a limit, is refused as it is taken in, SQLCODE -900, a
/// value or integers at a time.
#[test]
fn a_sum_past_128_bits_is_refused() {
    let half = Value::Exact(Decimal::new((i128::MAX >> 1_u32) + 1_i128, 0));
    let mut sum = Sum.accumulator(WHOLE);
    sum.update(&half).unwrap();
    assert_eq!(sum.update(&half).unwrap_err().sqlcode(), -900_i32);
    let mut ints = Sum.accumulator(WHOLE);
    ints.update(&Value::Exact(Decimal::new(i128::MAX - 1_i128, 0))).unwrap();
    let err = ints.update_ints(&Ints::new(vec![5_i64], 0, None), None).unwrap_err();
    assert_eq!(err.sqlcode(), -900_i32);
}

/// Verifies: [DESIGN] RULES.md D15 [ISO/IEC 9075:1992 §6.5 GR2b iv]: SUM
/// of approximate numbers whose total is in range gives it in every
/// order, though two of them overflow an `f64`; AVG of such is their mean;
/// a total out of range is 22003; values below 2^-900 are summed too, and
/// -0s give -0.
#[test]
fn an_approximate_sum_is_judged_as_a_total() {
    let fits = [Value::Approx(f64::MAX), Value::Approx(f64::MAX), Value::Approx(-f64::MAX)];
    for order in ORDERS {
        for got in finished(&Sum, DOUBLE, &fits, order) {
            assert_eq!(got, format!("Ok({:?})", Value::Approx(f64::MAX).to_string()), "SUM in {order:?}");
        }
    }
    let mut avg = Avg.accumulator(DOUBLE);
    avg.update_many(&fits[..2], None).unwrap();
    assert_eq!(avg.finish().unwrap().to_string(), Value::Approx(f64::MAX).to_string());
    let mut past = Sum.accumulator(DOUBLE);
    past.update_many(&fits[..2], None).unwrap();
    assert_eq!(past.finish().unwrap_err().sqlstate().to_string(), "22003");
    let tiny = 1.0e-300_f64;
    let mut small = Sum.accumulator(DOUBLE);
    small.update_many(&[Value::Approx(tiny), Value::Approx(tiny)], None).unwrap();
    assert_eq!(bits(&*small), Some(2.0e-300_f64.to_bits()));
    let mut zeros = Sum.accumulator(DOUBLE);
    zeros.update_many(&[Value::Approx(-0.0_f64), Value::Approx(-0.0_f64)], None).unwrap();
    assert_eq!(bits(&*zeros), Some((-0.0_f64).to_bits()));
}

/// Verifies: [X3.135-1986 §5.9 SR5] a sum of exact and approximate values
/// is approximate, either first; of exact values first and approximate
/// ones as a vector after; AVG of exact values of an approximate result
/// is their quotient.
#[test]
fn exact_and_approximate_values_sum_approximately() {
    let (two, half) = (Value::Exact(Decimal::new(2_i128, 0)), Value::Approx(0.5_f64));
    for values in [[two.clone(), half.clone()], [half, two.clone()]] {
        let mut sum = Sum.accumulator(DOUBLE);
        values.iter().try_for_each(|v| sum.update(v)).unwrap();
        assert_eq!(bits(&*sum), Some(2.5_f64.to_bits()));
    }
    let mut vector = Sum.accumulator(DOUBLE);
    vector.update(&two).unwrap();
    vector.update_floats(&Floats::new(vec![0.5_f64], None), None).unwrap();
    assert_eq!(bits(&*vector), Some(2.5_f64.to_bits()));
    let mut avg = Avg.accumulator(DOUBLE);
    avg.update_many(&[two.clone(), two], None).unwrap();
    assert_eq!(avg.finish().unwrap().to_string(), "2");
}

/// Verifies: [DESIGN] RULES.md D15 [ISO/IEC 9075:1992 §6.5 GR2b iv,
/// SR9e]: SUM of intervals whose sum on the way leaves 9 digits of days,
/// but whose total does not, gives it in every order; AVG their mean; a
/// total its fields cannot hold is 22015; one 128 bits cannot, -900.
#[test]
fn an_interval_sum_is_judged_as_a_total() {
    let q = Qualifier::new(Field::Day, Field::Day, 9, 0);
    let day = crate::value::interval::unit(Field::Day);
    let days = |n: i128| Value::Interval(Interval { units: n * day, qualifier: q });
    let most = pow10(9).unwrap() - 1_i128;
    for order in ORDERS {
        for got in finished(&Sum, DataType::Interval(q), &[days(most), days(1_i128), days(-1_i128)], order) {
            assert_eq!(got, format!("Ok({:?})", days(most).to_string()), "SUM in {order:?}");
        }
    }
    let mut avg = Avg.accumulator(DataType::Interval(q));
    avg.update_many(&[days(most), days(most)], None).unwrap();
    assert_eq!(avg.finish().unwrap().to_string(), days(most).to_string());
    let mut past = Sum.accumulator(DataType::Interval(q));
    past.update_many(&[days(most), days(1_i128)], None).unwrap();
    assert_eq!(past.finish().unwrap_err().sqlstate().to_string(), "22015");
    let huge = Value::Interval(Interval { units: i128::MAX, qualifier: q });
    let mut bits = Sum.accumulator(DataType::Interval(q));
    bits.update(&huge).unwrap();
    assert_eq!(bits.update(&huge).unwrap_err().sqlcode(), -900_i32);
}

/// Verifies: [DESIGN] (robustness, R-TEST-2) values of no number or
/// interval, which a plug-in may give, are summed as `arith` adds them:
/// the first kept, the next refused, as is a number after an interval.
#[test]
fn other_values_are_summed_by_arith() {
    let mut sum = Sum.accumulator(WHOLE);
    sum.update(&Value::Char("a".into())).unwrap();
    assert_eq!(sum.update(&Value::Char("b".into())).unwrap_err().code, Code::Data);
    let mut lone = Sum.accumulator(WHOLE);
    lone.update(&Value::Char("a".into())).unwrap();
    lone.update(&Value::Null).unwrap();
    assert_eq!(lone.finish().unwrap().to_string(), "a");
    let q = Qualifier::new(Field::Day, Field::Day, 2, 0);
    let mut mixed = Sum.accumulator(DataType::Interval(q));
    mixed.update(&Value::Interval(Interval { units: 0_i128, qualifier: q })).unwrap();
    mixed.update_ints(&Ints::new(vec![1_i64], 0, None), None).unwrap();
    mixed.update_floats(&Floats::new(vec![1.0_f64], None), None).unwrap();
    let mut empty = Sum.accumulator(WHOLE);
    empty.update_ints(&Ints::new(Vec::new(), 0, None), None).unwrap();
    assert!(empty.finish().unwrap().is_null());
}
