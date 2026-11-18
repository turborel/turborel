//! Geometry projections, candidate contracts and reference-bound locators.

use super::*;
use crate::functions::ScalarRef;
use crate::geometry::{Geometry, GeometryFunction, data_type, data_type_with_srid};

fn shape(text: &str, srid: i32) -> Key {
    Key(vec![Geometry::from_wkt(text, srid).unwrap().to_value()])
}

fn intersects(value: Value) -> Probe {
    Probe::Predicate {
        predicate: Box::new(Predicate { function: ScalarRef(Arc::new(GeometryFunction::Intersects)), column: 0, argument: Some(0) }),
        value,
    }
}

/// Verifies: [DESIGN] D85: projection collisions retain one counted entry
/// per envelope/row pair. Equality returns candidates, not original-key
/// equality; removing one version retains another with the same envelope.
#[test]
fn geometry_projection_collisions_are_counted_candidates() {
    let method = RTreeMethod::in_memory();
    let types = [data_type_with_srid(4326).unwrap()];
    let index: Arc<dyn Index> = Arc::from(method.create(&types).unwrap());
    let first = shape("LINESTRING (0 0, 4 4)", 4326);
    let second = shape("LINESTRING (0 4, 4 0)", 4326);
    index.insert(first.clone(), 7).unwrap();
    index.insert(second.clone(), 7).unwrap();
    index.insert(second.clone(), 8).unwrap();
    let projection = index.key_projection().unwrap();
    assert_eq!(projection.project(&first).unwrap(), projection.project(&second).unwrap());
    let entries: Vec<_> = Arc::clone(&index).search(&Probe::All, KeyOrder::Ascending).map(Result::unwrap).collect();
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().all(|e| e.0 == projection.project(&first).unwrap()));
    assert_eq!(index.rows(&first).unwrap(), [7, 8]);
    assert_eq!(Arc::clone(&index).key_rows(&first).map(Result::unwrap).collect::<Vec<_>>(), [7, 8]);
    index.remove(&first, 7).unwrap();
    assert_eq!(ids(&index, &Probe::All), [7, 8]);
    index.remove(&second, 7).unwrap();
    assert_eq!(ids(&index, &Probe::All), [8]);
    assert_eq!(projection.project(&Key(vec![Value::Null])).unwrap(), Key(vec![Value::Null]));
    assert_eq!(index.rows(&Key(vec![Value::Null])).unwrap(), []);
    assert_eq!(index.insert(shape("POINT EMPTY", 0), 9).unwrap_err().code, Code::Data);
    assert_eq!(index.insert(Key(vec![]), 9).unwrap_err().code, Code::Unsupported);
    assert_eq!(index.insert(Key(vec![Value::Null]), 9).unwrap_err().code, Code::Data);
    assert_eq!(ids(&index, &Probe::All), [8]);
}

/// Verifies: [DESIGN] D15, D85: a matching reference permits envelope
/// pruning; a changed reference must return every entry, including EMPTY,
/// so runtime SQL still reports mismatches. NULL never matches. A hole's
/// bounding box is only a candidate and cannot claim exact intersection.
#[test]
fn geometry_candidates_preserve_empty_and_reference_errors() {
    let method = RTreeMethod::in_memory();
    let index: Arc<dyn Index> = Arc::from(method.create(&[data_type_with_srid(4326).unwrap()]).unwrap());
    for (row, text) in
        [(1, "POINT EMPTY"), (2, "POINT (100 100)"), (3, "POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (2 2, 2 8, 8 8, 8 2, 2 2))")]
    {
        index.insert(shape(text, 4326), row).unwrap();
    }
    for (text, srid, expected) in [
        ("POINT (5 5)", 4_326_i32, vec![3]),
        ("POINT (100 100)", 4_326_i32, vec![2]),
        ("POINT EMPTY", 4_326_i32, vec![]),
        ("POINT (1000 1000)", 0_i32, vec![1, 2, 3]),
        ("POINT EMPTY", 0_i32, vec![1, 2, 3]),
    ] {
        let probe = intersects(Geometry::from_wkt(text, srid).unwrap().to_value());
        assert_eq!(ids(&index, &probe), expected);
        let count = if srid == 4_326_i32 { Some(expected.len()) } else { None };
        assert_eq!(index.estimate(&probe), count);
    }
    assert_eq!(ids(&index, &intersects(Value::Null)), []);
    assert_eq!(index.estimate(&intersects(Value::Null)), Some(0));
    let invalid = intersects(Value::Char("not geometry".into()));
    assert_eq!(index.estimate(&invalid), None);
    assert_eq!(Arc::clone(&index).search(&invalid, KeyOrder::Ascending).next().unwrap().unwrap_err().code, Code::Data);
    let compare = Probe::Compare { op: CompOp::Eq, value: Value::Null };
    assert_eq!(index.search(&compare, KeyOrder::Ascending).next().unwrap().unwrap_err().code, Code::Unsupported);
}

/// Verifies: [DESIGN] D85: geometry support is restricted to a fixed SRID
/// and the promised intersection function, on the sole key column in
/// either argument position. SQL equality/order capabilities stay off.
#[test]
fn geometry_capabilities_do_not_claim_original_key_order_or_equality() {
    let method = RTreeMethod::in_memory();
    let types = [data_type_with_srid(4326).unwrap()];
    assert_eq!(method.create(&[data_type()]).err().unwrap().code, Code::Unsupported);
    assert!(!method.supports_compare(CompOp::Eq, &types));
    assert!(!method.supports_prefix(1, 1, None));
    assert!(!method.supports_range(1, 0));
    assert!(!method.supports_columns(1, &[(0, CompOp::Eq)]));
    let Probe::Predicate { mut predicate, .. } = intersects(Value::Null) else { panic!("predicate") };
    assert!(method.supports_predicate(&predicate, &types));
    predicate.argument = Some(1);
    assert!(method.supports_predicate(&predicate, &types));
    predicate.argument = Some(2);
    assert!(!method.supports_predicate(&predicate, &types));
    predicate.argument = Some(0);
    predicate.column = 1;
    assert!(!method.supports_predicate(&predicate, &types));
    predicate.column = 0;
    assert!(!method.supports_predicate(&predicate, &[data_type()]));
    predicate.function = ScalarRef(Arc::new(GeometryFunction::Disjoint));
    assert!(!method.supports_predicate(&predicate, &types));
    let index: Arc<dyn Index> = Arc::from(method.create(&types).unwrap());
    let unsupported = Probe::Predicate { predicate, value: Value::Null };
    assert_eq!(index.search(&unsupported, KeyOrder::Ascending).next().unwrap().unwrap_err().code, Code::Unsupported);
}

/// Verifies: [DESIGN] D85: geometry locators bind their SRID, preserve
/// projected version counts across reopen, and refuse wrong versions,
/// references, truncations and trailing bytes. Numeric v1 stays readable.
#[test]
fn geometry_locators_bind_reference_and_preserve_counts() {
    let method = RTreeMethod::in_memory();
    let types = [data_type_with_srid(4326).unwrap()];
    let index: Arc<dyn Index> = Arc::from(method.create(&types).unwrap());
    let key = shape("POINT EMPTY", 4326);
    index.insert(key.clone(), 7).unwrap();
    index.insert(key.clone(), 7).unwrap();
    let locator = index.locator();
    assert_eq!(locator[0], GEOMETRY_LOCATOR_VERSION);
    method.checkpoint().unwrap();
    drop(index);
    let opened: Arc<dyn Index> = Arc::from(method.open(&types, &locator).unwrap().unwrap());
    opened.remove(&key, 7).unwrap();
    assert_eq!(ids(&opened, &Probe::All), [7]);
    assert!(opened.key_projection().is_some());
    for length in 0..locator.len() {
        assert_eq!(method.open(&types, &locator[..length]).err().unwrap().code, Code::Corrupt);
    }
    assert_eq!(method.open(&[data_type_with_srid(0).unwrap()], &locator).err().unwrap().code, Code::Corrupt);
    let mut wrong = locator.clone();
    wrong[0] = LOCATOR_VERSION;
    assert_eq!(method.open(&types, &wrong).err().unwrap().code, Code::Corrupt);
    let mut trailing = locator;
    trailing.push(0);
    assert_eq!(method.open(&types, &trailing).err().unwrap().code, Code::Corrupt);
    let numeric = method.create(&columns(2)).unwrap();
    assert_eq!(numeric.locator()[0], LOCATOR_VERSION);
    assert!(method.open(&columns(2), &numeric.locator()).unwrap().is_some());
}
