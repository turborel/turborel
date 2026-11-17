use super::*;

/// The span from point `a` to point `b`, as numbers: they compare as
/// points in time do.
fn days(a: i64, b: i64) -> Span {
    let day = |d: i64| Value::Exact(crate::value::Decimal::new(i128::from(d), 0));
    (day(a), day(b))
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.25.3.3] a span is covered by spans,
/// in order of their starts, that meet or overlap from its start to its
/// end, one or several; not by spans that leave a gap, start after it, or
/// end before it ends; nor by none.
#[test]
fn a_span_is_covered_only_without_a_gap() {
    let covered = |spans: &[Span], span: Span| cover(spans.iter().cloned(), &span);
    assert!(covered(&[days(1, 31)], days(5, 10)), "within one");
    assert!(covered(&[days(1, 5), days(5, 10)], days(2, 9)), "two that meet");
    assert!(covered(&[days(1, 6), days(3, 4), days(4, 12)], days(2, 11)), "overlapping, one within another");
    assert!(!covered(&[days(1, 5), days(6, 10)], days(2, 9)), "a gap");
    assert!(!covered(&[days(3, 10)], days(2, 9)), "starting after it");
    assert!(!covered(&[days(1, 8)], days(2, 9)), "ending before it");
    assert!(!covered(&[], days(2, 9)), "none");
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.16.2] spans overlap when each starts
/// before the other ends: not when they meet.
#[test]
fn spans_overlap_unless_they_meet_or_are_apart() {
    assert!(overlap(&days(1, 5), &days(4, 9)));
    assert!(overlap(&days(1, 9), &days(4, 5)));
    assert!(!overlap(&days(1, 5), &days(5, 9)));
    assert!(!overlap(&days(6, 9), &days(1, 5)));
}
