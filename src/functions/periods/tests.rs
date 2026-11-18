use super::*;
use crate::value::Decimal;

/// The truth value `v` is: none of the null value.
fn t(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::Null => None,
        Value::Char(_)
        | Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Binary(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Lob(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Row(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Json(_)
        | Value::Extension(_) => panic!("{v} is no truth value"),
    }
}

/// The instant `n`: a whole number stands for one, as the predicates
/// only order their bounds.
fn n(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// The predicate `op` of periods neither written, from `a` to `b` and
/// from `c` to `d`.
fn of(op: PeriodOperator, (a, b): (i128, i128), (c, d): (i128, i128)) -> Value {
    PeriodTest { op, written: [false, false] }.call(&[n(a), n(b), n(c), n(d)]).unwrap()
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.20 GR4b iv]
/// OVERLAPS, MC/DC of SV1 < EV2 and EV1 > SV2: both true, true (a shared
/// instant); the first false alone, false (the first after the second);
/// the second false alone, false (the first ending where the second
/// starts).
#[test]
fn overlaps_shows_mc_dc() {
    assert_eq!(t(&of(PeriodOperator::Overlaps, (1, 5), (4, 9))), Some(true));
    assert_eq!(t(&of(PeriodOperator::Overlaps, (9, 12), (4, 9))), Some(false));
    assert_eq!(t(&of(PeriodOperator::Overlaps, (1, 4), (4, 9))), Some(false));
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.20 GR4b v]
/// EQUALS, MC/DC of SV1 = SV2 and EV1 = EV2: both, true; the starts
/// apart alone, false; the ends apart alone, false.
#[test]
fn equals_shows_mc_dc() {
    assert_eq!(t(&of(PeriodOperator::Equals, (1, 5), (1, 5))), Some(true));
    assert_eq!(t(&of(PeriodOperator::Equals, (2, 5), (1, 5))), Some(false));
    assert_eq!(t(&of(PeriodOperator::Equals, (1, 6), (1, 5))), Some(false));
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.20 GR4a v 3]
/// CONTAINS of a period, MC/DC of SV1 ≤ SV2 and EV1 ≥ EV2: both, true,
/// at equal bounds too; the start after alone, false; the end before
/// alone, false.
#[test]
fn contains_a_period_shows_mc_dc() {
    assert_eq!(t(&of(PeriodOperator::Contains, (1, 9), (1, 9))), Some(true));
    assert_eq!(t(&of(PeriodOperator::Contains, (1, 9), (3, 5))), Some(true));
    assert_eq!(t(&of(PeriodOperator::Contains, (2, 9), (1, 5))), Some(false));
    assert_eq!(t(&of(PeriodOperator::Contains, (1, 4), (1, 5))), Some(false));
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.20 GR4a v 1, v 2]
/// CONTAINS of a point in time, MC/DC of SV1 ≤ VE and EV1 > VE: the
/// start itself, true; before the start, false; the end, excluded,
/// false; a null point, Unknown.
#[test]
fn contains_a_point_shows_mc_dc() {
    let contains =
        |a: i128, b: i128, v: Value| PeriodTest { op: PeriodOperator::Contains, written: [false, false] }.call(&[n(a), n(b), v]).unwrap();
    assert_eq!(t(&contains(1, 5, n(1))), Some(true));
    assert_eq!(t(&contains(1, 5, n(0))), Some(false));
    assert_eq!(t(&contains(1, 5, n(5))), Some(false));
    assert_eq!(t(&contains(1, 5, Value::Null)), None);
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.20 GR4b vi to ix]
/// PRECEDES and SUCCEEDS hold of periods that meet or are apart;
/// IMMEDIATELY PRECEDES and SUCCEEDS of periods that meet alone.
#[test]
fn precedes_and_succeeds_meet_or_part() {
    use PeriodOperator::{ImmediatelyPrecedes, ImmediatelySucceeds, Precedes, Succeeds};
    let cases = [
        (Precedes, (1, 4), (4, 9), true),
        (Precedes, (1, 3), (4, 9), true),
        (Precedes, (1, 5), (4, 9), false),
        (Succeeds, (4, 9), (1, 4), true),
        (Succeeds, (5, 9), (1, 4), true),
        (Succeeds, (3, 9), (1, 4), false),
        (ImmediatelyPrecedes, (1, 4), (4, 9), true),
        (ImmediatelyPrecedes, (1, 3), (4, 9), false),
        (ImmediatelySucceeds, (4, 9), (1, 4), true),
        (ImmediatelySucceeds, (5, 9), (1, 4), false),
    ];
    for (op, first, second, holds) in cases {
        assert_eq!(t(&of(op, first, second)), Some(holds), "{op:?} {first:?} {second:?}");
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.20 GR2a]
/// A written period of a null bound makes the predicate Unknown, MC/DC of
/// its two conditions: the first period's, the second's, and neither. A
/// period not written, as of an outer join's nulls, makes it False.
#[test]
fn a_written_null_bound_is_unknown() {
    let test = |written: [bool; 2], args: &[Value]| PeriodTest { op: PeriodOperator::Overlaps, written }.call(args).unwrap();
    assert_eq!(t(&test([true, true], &[Value::Null, n(5), n(1), n(9)])), None);
    assert_eq!(t(&test([true, true], &[n(1), n(5), n(1), Value::Null])), None);
    assert_eq!(t(&test([true, true], &[n(1), n(5), n(1), n(9)])), Some(true));
    assert_eq!(t(&test([false, false], &[Value::Null, n(5), n(1), n(9)])), Some(false));
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.20 GR2b]
/// A written period that does not start before it ends fails (22020),
/// MC/DC of written and start ≥ end: both, the error, also of a start at
/// its end; not written, no error; written in order, none. A null in
/// the other written period comes first [GR2a].
#[test]
fn a_written_period_out_of_order_fails() {
    let test = |written: [bool; 2], args: &[Value]| PeriodTest { op: PeriodOperator::Equals, written }.call(args);
    for (a, b) in [(5, 1), (5, 5)] {
        let e = test([true, false], &[n(a), n(b), n(1), n(9)]).unwrap_err();
        assert_eq!(e.sqlstate(), SqlState::INVALID_PERIOD_VALUE, "{e}");
    }
    assert_eq!(t(&test([false, false], &[n(5), n(1), n(1), n(9)]).unwrap()), Some(false));
    assert_eq!(t(&test([true, true], &[n(1), n(9), n(1), n(9)]).unwrap()), Some(true));
    assert_eq!(t(&test([true, true], &[n(5), n(1), Value::Null, n(9)]).unwrap()), None);
}

/// Verifies: [DESIGN] RULES.md R-TEST-1
/// Each predicate is named for a plan and returns BOOLEAN.
#[test]
fn each_predicate_is_named() {
    let test = PeriodTest { op: PeriodOperator::ImmediatelySucceeds, written: [false, false] };
    assert_eq!((test.name(), test.return_type(&[]).unwrap()), ("PERIOD IMMEDIATELY SUCCEEDS", DataType::Boolean));
}
