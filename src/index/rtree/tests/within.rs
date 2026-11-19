//! Distance and relate windows: what they find, and where they must read
//! every entry so a scan's error is still raised (D15, D135).

use super::*;
use crate::functions::ScalarRef;
use crate::geometry::routines::Routine;
use crate::geometry::{Geometry, GeometryFunction, data_type_with_srid};
use crate::spatial::window;

fn point(x: f64, y: f64, srid: i32) -> Value {
    Geometry::point(x, y, srid).unwrap().to_value()
}

fn probe(function: ScalarRef, value: Value) -> Probe {
    Probe::Predicate { predicate: Box::new(Predicate { function, column: 0, argument: Some(1) }), value }
}

fn within(geometry: Value, limit: Value, scale: f64) -> Probe {
    probe(ScalarRef(Arc::new(GeometryFunction::Distance)), window::within(geometry, limit, scale))
}

fn related(geometry: Value, pattern: &str) -> Probe {
    probe(ScalarRef(Arc::new(Routine::Relate)), window::related(geometry, Value::Char(pattern.into())))
}

/// Points 1 to 20 at (n, 0), an empty point as 21, all of SRID 0.
fn line() -> Arc<dyn Index> {
    let index: Arc<dyn Index> = Arc::from(RTreeMethod::in_memory().create(&[data_type_with_srid(0).unwrap()]).unwrap());
    for n in 1..=20_u32 {
        index.insert(Key(vec![point(f64::from(n), 0.0, 0)]), u64::from(n)).unwrap();
    }
    index.insert(Key(vec![Geometry::from_wkt("POINT EMPTY", 0).unwrap().to_value()]), 21).unwrap();
    index
}

/// Verifies: [DESIGN] D135: a distance window finds every entry whose
/// envelope is within the limit, in the unit's scale, its bound inclusive,
/// and not one in its window's corner beyond the limit; a null or negative
/// limit finds none.
#[test]
fn distance_windows_find_within_their_limits() {
    let index = line();
    index.insert(Key(vec![point(11.8, 1.8, 0)]), 30).unwrap();
    let near = |limit: Value, scale: f64| ids(&index, &within(point(10.0, 0.0, 0), limit, scale));
    assert_eq!(near(Value::Approx(2.0), 1.0_f64), [8, 9, 10, 11, 12]);
    assert_eq!(near(Value::Approx(0.0), 1.0_f64), [10]);
    assert_eq!(near(Value::Approx(2.0), 0.5_f64), [6, 7, 8, 9, 10, 11, 12, 13, 14, 30]);
    for none in [Value::Null, Value::Approx(-1.0)] {
        assert_eq!(near(none, 1.0_f64), Vec::<RowId>::new());
    }
}

/// Verifies: [DESIGN] D15, D135: a distance window of another SRID, an
/// infinite or non-numeric limit or a malformed ROW reads every entry, so
/// the recheck raises what a scan would; one of a null or empty geometry
/// reads none. Its estimate is none where it reads every entry.
#[test]
fn distance_windows_read_every_entry_where_a_scan_raises() {
    let index = line();
    let near = |limit: Value| ids(&index, &within(point(10.0, 0.0, 0), limit, 1.0_f64));
    let every: Vec<RowId> = (1..=21).collect();
    assert_eq!(near(Value::Approx(f64::INFINITY)), every);
    assert_eq!(near(Value::Char("x".into())), every);
    assert_eq!(ids(&index, &within(point(10.0, 0.0, 4326), Value::Approx(2.0), 1.0_f64)), every);
    assert_eq!(ids(&index, &within(point(10.0, 0.0, 4326), Value::Null, 1.0)), every);
    assert_eq!(ids(&index, &within(Value::Null, Value::Approx(2.0), 1.0_f64)), Vec::<RowId>::new());
    let empty = Geometry::from_wkt("POINT EMPTY", 0).unwrap().to_value();
    assert_eq!(ids(&index, &within(empty, Value::Approx(2.0), 1.0_f64)), Vec::<RowId>::new());
    let malformed = probe(ScalarRef(Arc::new(GeometryFunction::Distance)), point(10.0, 0.0, 0));
    assert_eq!(ids(&index, &malformed), every);
    assert_eq!(index.estimate(&within(point(10.0, 0.0, 0), Value::Approx(2.0), 1.0_f64)), Some(5));
    assert_eq!(index.estimate(&within(point(10.0, 0.0, 0), Value::Null, 1.0)), Some(0));
    assert_eq!(index.estimate(&within(point(10.0, 0.0, 4326), Value::Approx(2.0), 1.0_f64)), None);
}

/// Verifies: [DESIGN] D15, D135: a tree holding an entry beyond the
/// finite-distance domain cannot certify a window, so it reads every
/// entry, even of a null limit, and the overflowing distance is computed.
#[test]
fn uncertified_distance_windows_read_every_entry() {
    let index = line();
    index.insert(Key(vec![point(1.7e308, 0.0, 0)]), 99).unwrap();
    let mut every: Vec<RowId> = (1..=21).collect();
    every.push(99);
    assert_eq!(ids(&index, &within(point(10.0, 0.0, 0), Value::Approx(2.0), 1.0_f64)), every);
    assert_eq!(ids(&index, &within(point(10.0, 0.0, 0), Value::Null, 1.0)), every);
    let far = line();
    assert_eq!(ids(&far, &within(point(-1.7e308, 0.0, 0), Value::Approx(2.0), 1.0_f64)).len(), 21);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.45] D135: a
/// relate window reads the envelope window of a pattern asking an
/// interior or boundary to meet, every entry of one that does not, and
/// none of a null geometry or pattern or an empty geometry.
#[test]
fn relate_windows_read_their_patterns() {
    let index = line();
    let at = point(10.0, 0.0, 0);
    assert_eq!(ids(&index, &related(at.clone(), "T********")), [10]);
    assert_eq!(ids(&index, &related(at.clone(), "FF*FF****")).len(), 21);
    assert_eq!(ids(&index, &related(at.clone(), "not a pattern")).len(), 21);
    assert_eq!(ids(&index, &related(point(10.0, 0.0, 4326), "T********")).len(), 21);
    assert_eq!(ids(&index, &related(Value::Null, "T********")), Vec::<RowId>::new());
    let empty = Geometry::from_wkt("POINT EMPTY", 0).unwrap().to_value();
    assert_eq!(ids(&index, &related(empty, "T********")), Vec::<RowId>::new());
    let null_pattern = probe(ScalarRef(Arc::new(Routine::Relate)), window::related(at, Value::Null));
    assert_eq!(ids(&index, &null_pattern), Vec::<RowId>::new());
    let method = RTreeMethod::in_memory();
    let Probe::Predicate { predicate, .. } = related(Value::Null, "T********") else { panic!("predicate") };
    assert!(method.supports_predicate(&predicate, &[data_type_with_srid(0).unwrap()]));
}
