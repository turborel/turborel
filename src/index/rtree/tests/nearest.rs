//! Geometry distance capability, projection, fallback and cursor lifetime.

use super::*;
use crate::functions::{ScalarFunction, ScalarRef};
use crate::geometry::{Geometry, GeometryFunction, SpatialOperation, data_type, data_type_with_srid};

fn query(value: Value) -> NearestQuery {
    NearestQuery {
        function: ScalarRef(Arc::new(GeometryFunction::Distance)),
        column: 0,
        argument: 0,
        value,
        descending: false,
        statistics: None,
    }
}

struct Unstable;

impl ScalarFunction for Unstable {
    fn name(&self) -> &'static str {
        "UNSTABLE"
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        GeometryFunction::Distance.return_type(args)
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        GeometryFunction::Distance.call(args)
    }
    fn deterministic(&self) -> bool {
        false
    }
    fn spatial_operation(&self) -> Option<SpatialOperation> {
        Some(SpatialOperation::Distance)
    }
}

/// Verifies: [DESIGN] D85: a distance capability needs a single fixed-SRID
/// geometry key and complete deterministic distance semantics. MC/DC:
/// each function declaration, key/call position and column type can refuse
/// support independently; ordinary index methods default to unsupported.
#[test]
fn nearest_capability_requires_the_complete_distance_contract() {
    let method = RTreeMethod::in_memory();
    let types = [data_type_with_srid(0).unwrap()];
    let request = query(Value::Null);
    assert!(method.supports_nearest(&request, &types));
    assert!(method.supports_nearest(&NearestQuery { argument: 1, ..request.clone() }, &types));
    for changed in [
        NearestQuery { column: 1, ..request.clone() },
        NearestQuery { argument: 2, ..request.clone() },
        NearestQuery { function: ScalarRef(Arc::new(GeometryFunction::Intersects)), ..request.clone() },
        NearestQuery { function: ScalarRef(Arc::new(Unstable)), ..request.clone() },
    ] {
        assert!(!method.supports_nearest(&changed, &types));
    }
    for unsupported in [vec![], vec![data_type()], vec![approximate()], columns(2)] {
        assert!(!method.supports_nearest(&request, &unsupported));
    }
    assert!(!crate::index::BTreeMethod::in_memory().supports_nearest(&request, &types));
}

/// Verifies: [DESIGN] D15, D85: malformed, NULL, empty, mixed-reference
/// and out-of-range requests decline before yielding, leaving runtime
/// scalar evaluation to the scan. Out-of-range stored data also declines.
#[test]
fn uncertified_distances_fall_back_without_eager_scalar_errors() {
    let method = RTreeMethod::in_memory();
    let index: Arc<dyn Index> = Arc::from(method.create(&[data_type_with_srid(0).unwrap()]).unwrap());
    let budget = MemoryBudget::new(usize::MAX);
    for value in [
        Value::Null,
        Value::Char("not a geometry".into()),
        Geometry::from_wkt("POINT EMPTY", 0).unwrap().to_value(),
        Geometry::point(0.0_f64, 0.0_f64, 1).unwrap().to_value(),
        Geometry::point(f64::MAX, 0.0_f64, 0).unwrap().to_value(),
    ] {
        assert!(Arc::clone(&index).nearest(&query(value), &budget).unwrap().is_none());
    }
    let request = query(Geometry::point(0.0_f64, 0.0_f64, 0).unwrap().to_value());
    assert_eq!(Arc::clone(&index).nearest(&request, &budget).unwrap().unwrap().count(), 0);
    index.insert(Key(vec![Geometry::point(f64::MAX, 0.0_f64, 0).unwrap().to_value()]), 1).unwrap();
    assert!(Arc::clone(&index).nearest(&request, &budget).unwrap().is_none());
    let numeric: Arc<dyn Index> = Arc::from(method.create(&columns(2)).unwrap());
    assert!(numeric.nearest(&request, &budget).unwrap().is_none());
}

/// Verifies: [DESIGN] D38, D85: nearest candidates keep their projected
/// key and index alive, include each counted pair once, and expose bounds
/// rather than claiming a polygon hole has distance zero. The frontier
/// is budgeted and released; exhausting it reports SQLCODE -918.
#[test]
fn nearest_candidates_preserve_projection_counts_budget_and_lifetime() {
    let method = RTreeMethod::in_memory();
    let index: Arc<dyn Index> = Arc::from(method.create(&[data_type_with_srid(0).unwrap()]).unwrap());
    let geometries = [
        Geometry::point(8.0_f64, 9.0_f64, 0).unwrap(),
        Geometry::from_wkt("POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (4 4, 4 6, 6 6, 6 4, 4 4))", 0).unwrap(),
        Geometry::from_wkt("LINESTRING (0 10, 10 10)", 0).unwrap(),
    ];
    for (id, geometry) in (0_u64..).zip(&geometries) {
        index.insert(Key(vec![geometry.to_value()]), id).unwrap();
        index.insert(Key(vec![geometry.to_value()]), id).unwrap();
    }
    let point = Geometry::point(5.0_f64, 5.0_f64, 0).unwrap();
    let request = query(point.to_value());
    let budget = MemoryBudget::new(0);
    assert_eq!(Arc::clone(&index).nearest(&request, &budget).err().unwrap().code, Code::Resources);
    budget.set_limit(usize::MAX);
    let weak = Arc::downgrade(&index);
    let projection = index.key_projection().unwrap();
    let mut entries = Arc::clone(&index).nearest(&request, &budget).unwrap().unwrap();
    index.discard();
    drop(index);
    assert!(weak.upgrade().is_some());
    let found: Vec<_> = entries.by_ref().map(Result::unwrap).collect();
    assert_eq!(found.len(), geometries.len());
    assert_eq!(found[0].2, 1);
    assert_eq!(found[0].0.to_bits(), 0);
    assert_eq!(point.distance(&geometries[1]).unwrap(), Some(1.0_f64));
    for (bound, key, id) in found {
        let geometry = &geometries[usize::try_from(id).unwrap()];
        assert_eq!(key, projection.project(&Key(vec![geometry.to_value()])).unwrap());
        assert!(bound <= point.distance(geometry).unwrap().unwrap());
    }
    drop(entries);
    assert!(weak.upgrade().is_none());
    assert_eq!(budget.reserved(), 0);
}
