//! Constant-operand preparation preserves evaluation and plan reuse.

use super::common::dense_ring;
use super::*;
use crate::functions::ScalarFunction;
use crate::memory::MemoryBudget;

mod borrowed;
mod rectangles;

/// Verifies: [DESIGN] D15, D85: prepared point location preserves every
/// scalar result, including distance bits, holes, boundaries and other
/// geometry kinds. SRID errors and changed constant operands still take
/// their ordinary path, in either argument position.
#[test]
fn prepared_polygon_scalars_match_ordinary_evaluation() {
    let budget = MemoryBudget::new(usize::MAX);
    let polygon = Geometry::of(Shape::Polygon(vec![dense_ring(0.0, 10.0), dense_ring(2.0, 8.0)]), 0).unwrap().to_value();
    let filled = Geometry::rectangle([0.0_f64, 0.0_f64], [10.0_f64, 10.0_f64], 0).unwrap().to_value();
    for function in [GeometryFunction::Intersects, GeometryFunction::Disjoint, GeometryFunction::Distance] {
        for position in [0_usize, 1] {
            let mut arguments = [None, None];
            arguments[position] = Some(&polygon);
            let prepared = function.specialize(&arguments, &budget).unwrap();
            for text in [
                "POINT (0 0)",
                "POINT (1 1)",
                "POINT (2 5)",
                "POINT (5 5)",
                "POINT (10 5)",
                "POINT (11 11)",
                "POINT EMPTY",
                "LINESTRING (3 3, 7 7)",
                "POLYGON ((3 3, 7 3, 7 7, 3 3))",
                "MULTIPOINT ((1 1), (5 5))",
                "GEOMETRYCOLLECTION (POINT (5 5), POINT (12 12))",
            ] {
                let other = Geometry::from_wkt(text, 0).unwrap().to_value();
                for fixed in [&polygon, &filled] {
                    let mut pair = [other.clone(), other.clone()];
                    pair[position] = fixed.clone();
                    same_result(prepared.0.call(&pair).unwrap(), function.call(&pair).unwrap());
                }
            }
            for other in [Value::Null, Geometry::point(0.0, 0.0, 4326).unwrap().to_value()] {
                let mut pair = [other.clone(), other];
                pair[position] = polygon.clone();
                match function.call(&pair) {
                    Ok(expected) => same_result(prepared.0.call(&pair).unwrap(), expected),
                    Err(expected) => assert_eq!(prepared.0.call(&pair).unwrap_err().code, expected.code),
                }
            }
        }
    }
}

fn same_result(actual: Value, expected: Value) {
    match (actual, expected) {
        (Value::Approx(actual), Value::Approx(expected)) => assert_eq!(actual.to_bits(), expected.to_bits()),
        (actual, expected) => assert_eq!(actual.to_string(), expected.to_string()),
    }
}

/// Verifies: [DESIGN] D38, D85: an unaffordable interval index leaves
/// the decoded operand available. Its scalar results are unchanged and
/// the final clone releases both decoding and optional-index reservations.
#[test]
fn prepared_polygon_index_can_decline_without_losing_preparation() {
    let budget = MemoryBudget::new(usize::MAX);
    let value = Geometry::of(Shape::Polygon(vec![dense_ring(0.0, 10.0)]), 0).unwrap().to_value();
    let args = [None, Some(&value)];
    let prepared = GeometryFunction::Intersects.specialize(&args, &budget).unwrap();
    let full = budget.reserved();
    let reservations = budget.reservations();
    assert_eq!(reservations.len(), 2);
    let decoded = reservations[0].1;
    let shared = prepared.clone();
    drop(prepared);
    assert_eq!(budget.reserved(), full);
    drop(shared);
    assert_eq!(budget.reserved(), 0);
    budget.set_limit(decoded);
    let without_index = GeometryFunction::Intersects.specialize(&args, &budget).unwrap();
    assert_eq!(budget.reserved(), decoded);
    for point in [Geometry::point(1.0, 1.0, 0).unwrap(), Geometry::point(20.0, 20.0, 0).unwrap()] {
        let pair = [point.to_value(), value.clone()];
        same_result(without_index.0.call(&pair).unwrap(), GeometryFunction::Intersects.call(&pair).unwrap());
    }
    drop(without_index);
    assert_eq!(budget.reserved(), 0);
}

/// Verifies: [DESIGN] D15, D85: a prepared relationship retains all type
/// contracts and runtime behavior, including different arguments supplied
/// by a later plan execution. Preparing again reaches a fixed point.
#[test]
fn prepared_relationships_accept_changed_arguments() {
    let budget = MemoryBudget::new(usize::MAX);
    let fixed = Geometry::point(1.0, 1.0, 0).unwrap().to_value();
    let equal = Geometry::point(1.0, 1.0, 0).unwrap().to_value();
    let other = Geometry::point(2.0, 2.0, 0).unwrap().to_value();
    let mixed = Geometry::point(1.0, 1.0, 4326).unwrap().to_value();
    for function in [GeometryFunction::Intersects, GeometryFunction::Disjoint, GeometryFunction::Distance] {
        for arguments in [[Some(&fixed), None], [None, Some(&fixed)]] {
            let prepared = function.specialize(&arguments, &budget).unwrap();
            assert_eq!(prepared.0.name(), function.name());
            assert_eq!(
                prepared.0.return_type(&[data_type(), data_type()]).unwrap(),
                function.return_type(&[data_type(), data_type()]).unwrap()
            );
            assert!(prepared.0.specialize(&arguments, &budget).is_none());
            for position in 0..3 {
                assert_eq!(prepared.0.argument_type(position, &[]), function.argument_type(position, &[]));
                assert_eq!(prepared.0.parameter_type(position, &[]), function.parameter_type(position, &[]));
            }
            for pair in [
                [fixed.clone(), equal.clone()],
                [fixed.clone(), other.clone()],
                [other.clone(), other.clone()],
                [Value::Null, fixed.clone()],
                [fixed.clone(), Value::Null],
            ] {
                assert_eq!(prepared.0.call(&pair).unwrap().to_string(), function.call(&pair).unwrap().to_string());
            }
            assert!(prepared.0.call(&[]).is_err());
            assert_eq!(prepared.0.call(&[mixed.clone(), fixed.clone()]).unwrap_err().code, Code::Data);
        }
    }
}

/// Verifies: [DESIGN] D15, D85: no constant, NULL, an invalid value, or an
/// unsupported signature cannot raise a planning error or install a cache.
#[test]
fn failed_preparation_leaves_runtime_evaluation_intact() {
    let budget = MemoryBudget::new(usize::MAX);
    let text = Value::Char("not geometry".into());
    for args in [vec![], vec![None, None], vec![Some(&text), None], vec![None, Some(&Value::Null)]] {
        assert!(GeometryFunction::Intersects.specialize(&args, &budget).is_none());
    }
    assert!(GeometryFunction::Srid.specialize(&[None], &budget).is_none());
}

/// Verifies: [DESIGN] D38, D85: retained preparation reserves payload and
/// decoded storage, shares that reservation across clones and releases it
/// with the last owner. An insufficient budget declines preparation and
/// leaves ordinary evaluation available without changing its result.
#[test]
fn prepared_geometry_respects_and_releases_its_memory_budget() {
    let budget = MemoryBudget::new(usize::MAX);
    let point = Geometry::point(0.0, 0.0, 0).unwrap().to_value();
    let point_cache = GeometryFunction::Intersects.specialize(&[None, Some(&point)], &budget).unwrap();
    let base = budget.reserved() - GEOMETRY.encode_record(&point, &data_type()).unwrap().len();
    drop(point_cache);
    let value = Geometry::from_wkt("GEOMETRYCOLLECTION (POLYGON ((0 0, 3 0, 3 3, 0 0)), LINESTRING (1 1, 2 2), POINT (1 2))", 0)
        .unwrap()
        .to_value();
    let args = [None, Some(&value)];
    let prepared = GeometryFunction::Intersects.specialize(&args, &budget).unwrap();
    let bytes = budget.reserved();
    // Six coordinates live in ring/line allocations; the point is inline.
    assert!(bytes >= base + GEOMETRY.encode_record(&value, &data_type()).unwrap().len() + 6 * size_of::<[f64; 2]>());
    let shared = prepared.clone();
    drop(prepared);
    assert_eq!(budget.reserved(), bytes);
    drop(shared);
    assert_eq!(budget.reserved(), 0);
    for limit in [0, bytes - 1] {
        budget.set_limit(limit);
        assert!(GeometryFunction::Intersects.specialize(&args, &budget).is_none());
        assert_eq!(budget.reserved(), 0);
        assert_eq!(GeometryFunction::Intersects.call(&[value.clone(), value.clone()]).unwrap().to_string(), "1");
    }
}
