//! Unit tests of the inverse distribution functions' picks (RULES.md
//! R-TEST-1, R-TEST-8): each against GR10 by brute force, in exact
//! rational arithmetic, over every count of values to twelve and every
//! fraction of twentieths, of values with ties; approximate and DECFLOAT
//! fractions; a fraction out of range, null, and of no values.

use super::*;
use crate::value::{DOUBLE_PRECISION, DecFloat};

/// DOUBLE PRECISION, the result of an exact key.
const DOUBLE: DataType = DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::DoublePrecision };

/// The twentieths from 0 to 1.
const TWENTIETHS: i128 = 20;

/// How near two approximate numbers computed apart are taken as equal.
const NEAR: f64 = 1e-9_f64;

/// The fraction `num` / 20, exactly.
fn twentieths(num: i128) -> Value {
    Value::Exact(Decimal::new(num.checked_mul(5).unwrap(), 2))
}

/// Sorted values of `n` places, with ties: 0, 0, 2, 3, 3, 5, 6, 6, ...
fn sorted_values(n: u64) -> Vec<i128> {
    (0..i128::from(n)).map(|i| i.checked_sub(i.checked_rem(3).and_then(|r| r.checked_rem(2)).unwrap()).unwrap()).collect()
}

/// \[GR10h iii\] PERCENTILE_DISC by brute force: the least value whose
/// CUME_DIST, the values not after it over all, is at least num / 20.
fn disc(values: &[i128], num: i128) -> i128 {
    let n = i128::try_from(values.len()).unwrap();
    let cume_at_least = |v: &i128| {
        let not_after = i128::try_from(values.iter().filter(|w| *w <= v).count()).unwrap();
        not_after.checked_mul(TWENTIETHS).unwrap() >= num.checked_mul(n).unwrap()
    };
    *values.iter().filter(|v| cume_at_least(v)).min().unwrap()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR10h iii] of every
/// count to twelve and every twentieth, PERCENTILE_DISC picks the least
/// value whose CUME_DIST is at least the fraction, found by brute force,
/// a share equal to the fraction at least it, of values with ties.
///
/// Decision: what is left of NVE·N, nothing or more. Each changes the row
/// alone: a twentieth of a count that it divides, and of one it does not.
#[test]
fn disc_picks_the_first_value_of_a_cume_dist_at_least_10_9() {
    for n in 1..=12_u64 {
        let values = sorted_values(n);
        for num in 0..=TWENTIETHS {
            let p = picked(false, &twentieths(num), n).unwrap().unwrap();
            assert_eq!(p.low, p.high);
            let at = usize::try_from(p.low).unwrap();
            assert_eq!(values[at], disc(&values, num), "{n} values, {num}/20");
        }
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR10h ii] of every
/// count to twelve and every twentieth, PERCENTILE_CONT takes ROW0 =
/// ⌊NVE·(N−1)⌋ and ROW1 = ⌈NVE·(N−1)⌉, FACTOR what is between, by brute
/// force in exact arithmetic, and interpolates T0.Y + FACTOR·(T1.Y −
/// T0.Y) as an approximate number.
#[test]
fn cont_interpolates_between_its_rows_10_9() {
    for n in 1..=12_u64 {
        let values = sorted_values(n);
        let last = i128::from(n).checked_sub(1).unwrap();
        for num in 0..=TWENTIETHS {
            let times = num.checked_mul(last).unwrap();
            let (row0, left) = (times.checked_div(TWENTIETHS).unwrap(), times.checked_rem(TWENTIETHS).unwrap());
            let row1 = if left > 0 { row0.checked_add(1).unwrap() } else { row0 };
            let p = picked(true, &twentieths(num), n).unwrap().unwrap();
            assert_eq!((i128::from(p.low), i128::from(p.high)), (row0, row1), "{n} values, {num}/20");
            let factor = Decimal::new(left, 0).to_f64() / Decimal::new(TWENTIETHS, 0).to_f64();
            assert!((p.factor - factor).abs() < NEAR, "{n} values, {num}/20");
            let y = |r: i128| values[usize::try_from(r).unwrap()];
            let ys = (Value::Exact(Decimal::new(y(row0), 0)), Value::Exact(Decimal::new(y(row1), 0)));
            let Value::Approx(got) = picked_value(true, ys, p.factor, &DOUBLE).unwrap() else { panic!("not approximate") };
            let want = Decimal::new(y(row0), 0).to_f64() + factor * Decimal::new(y(row1).checked_sub(y(row0)).unwrap(), 0).to_f64();
            assert!((got - want).abs() < NEAR, "{n} values, {num}/20: {got} {want}");
        }
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR10b, GR10c, GR10h i]
/// a null fraction picks nothing, so null; one below 0 or above 1 is
/// SQLSTATE 22003, even of no values; no values pick nothing; approximate
/// and DECFLOAT fractions pick as exact ones do; PERCENTILE_DISC's value
/// is cast to the result's type.
#[test]
fn fractions_null_out_of_range_and_of_other_types_10_9() {
    assert_eq!(picked(true, &Value::Null, 4).unwrap(), None);
    assert_eq!(picked(false, &twentieths(10), 0).unwrap(), None);
    let state = |v: Value, n| picked(true, &v, n).err().and_then(|e| e.state);
    assert_eq!(state(Value::Exact(Decimal::new(-1, 2)), 3), Some(SqlState::NUMERIC_VALUE_OUT_OF_RANGE));
    assert_eq!(state(Value::Approx(1.5), 0), Some(SqlState::NUMERIC_VALUE_OUT_OF_RANGE));
    assert_eq!(state(Value::Approx(f64::NAN), 3), Some(SqlState::NUMERIC_VALUE_OUT_OF_RANGE));
    let half = Picked { low: 1, high: 2, factor: 0.5 };
    assert_eq!(picked(true, &Value::Approx(0.5), 4).unwrap(), Some(half));
    let decfloat = Value::DecFloat(DecFloat::of_decimal(&Decimal::new(5, 1)).unwrap());
    assert_eq!(picked(true, &decfloat, 4).unwrap(), Some(half));
    assert_eq!(picked(false, &Value::Approx(0.5), 4).unwrap(), Some(Picked { low: 1, high: 1, factor: 0.0 }));
    let disc = picked_value(false, (Value::Exact(Decimal::new(7, 0)), Value::Null), 0.0, &DOUBLE).unwrap();
    assert_eq!(disc.to_string(), Value::Approx(7.0).to_string());
    assert_eq!(Unfed.finish().err().map(|e| e.code), Some(Code::Api));
}
