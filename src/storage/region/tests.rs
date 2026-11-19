use super::*;
use crate::value::Decimal;

fn shape(text: &str) -> Value {
    Geometry::from_wkt(text, 0).unwrap().to_value()
}

fn window(low: [f64; 2], high: [f64; 2]) -> Bounds {
    Bounds::new(low, high).unwrap()
}

/// Verifies: [DESIGN] RULES.md D134: a region matches a row whose
/// geometry's envelope meets one of its envelopes, edges included; never
/// a null; an empty geometry as the origin an index keys it at; and a
/// value it cannot read as a geometry, or a row without the column, as
/// any row, so it is never narrower than the read.
#[test]
fn regions_match_rows_by_their_keyed_envelopes() {
    let region = Region::new(1, vec![window([0.0_f64, 0.0_f64], [10.0_f64, 10.0_f64]), window([20.0_f64, 20.0_f64], [30.0_f64, 30.0_f64])]);
    let row = |v: Value| vec![Value::Exact(Decimal::new(1, 0)), v];
    assert!(region.matches(&row(shape("POINT (5 5)"))), "inside the first");
    assert!(region.matches(&row(shape("POINT (30 25)"))), "on the second's edge");
    assert!(region.matches(&row(shape("LINESTRING (12 12, 40 40)"))), "an envelope crossing the second");
    assert!(!region.matches(&row(shape("POINT (15 15)"))), "between them");
    assert!(!region.matches(&row(Value::Null)), "a null");
    assert!(region.matches(&row(shape("POINT EMPTY"))), "an empty geometry at the origin, within the first");
    let away = Region::new(1, vec![window([1.0_f64, 1.0_f64], [2.0_f64, 2.0_f64])]);
    assert!(!away.matches(&row(shape("POINT EMPTY"))), "the origin outside");
    assert!(away.matches(&row(Value::Exact(Decimal::new(3, 0)))), "not a geometry");
    assert!(Region::new(5, Vec::new()).matches(&row(Value::Null)), "no such column");
    assert!(!Region::new(1, Vec::new()).matches(&row(shape("POINT (0 0)"))), "no envelope");
    assert_eq!((region.column(), region.envelopes().len()), (1, 2));
}

/// Verifies: [DESIGN] RULES.md D134: merging keeps one region a column,
/// of the hull of its envelopes, drops regions of no envelope, and keeps
/// columns apart.
#[test]
fn merged_regions_are_a_hull_of_each_column() {
    let regions = vec![
        Region::new(0, vec![window([0.0_f64, 0.0_f64], [1.0_f64, 1.0_f64])]),
        Region::new(2, vec![window([5.0_f64, 5.0_f64], [6.0_f64, 6.0_f64])]),
        Region::new(0, Vec::new()),
        Region::new(0, vec![window([3.0_f64, -2.0_f64], [4.0_f64, 0.5_f64]), window([-1.0_f64, 0.0_f64], [0.0_f64, 0.0_f64])]),
    ];
    let out = merged(regions);
    assert_eq!(out.len(), 2);
    assert_eq!((out[0].column(), out[0].envelopes()), (0, &[window([-1.0_f64, -2.0_f64], [4.0_f64, 1.0_f64])][..]));
    assert_eq!((out[1].column(), out[1].envelopes()), (2, &[window([5.0_f64, 5.0_f64], [6.0_f64, 6.0_f64])][..]));
    assert!(Region::new(0, Vec::new()).hull().is_none());
}
