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

/// Verifies: [DESIGN] D15, D85: once a prepared polygon searches its edges
/// for points' distances, every distance is the ordinary scan's to the
/// bit, for points inside, on, near and far from a comb and a holed
/// polygon, in either argument position.
#[test]
fn prepared_point_distances_match_the_scan_to_the_bit() {
    let budget = MemoryBudget::new(usize::MAX);
    let mut teeth = vec![[0.0_f64, 0.0_f64]];
    for t in 0..40_u32 {
        let x = f64::from(t) * 0.25_f64;
        teeth.extend([[x, 1.0_f64], [x, 9.0_f64], [x + 0.125_f64, 9.0_f64], [x + 0.125_f64, 1.0_f64]]);
    }
    teeth.extend([[10.0_f64, 1.0_f64], [10.0_f64, 0.0_f64], [0.0_f64, 0.0_f64]]);
    let comb = Geometry::of(Shape::Polygon(vec![teeth]), 0).unwrap().to_value();
    let holed = Geometry::of(Shape::Polygon(vec![dense_ring(0.0, 10.0), dense_ring(2.0, 8.0)]), 0).unwrap().to_value();
    for polygon in [comb, holed] {
        for position in [0_usize, 1] {
            let mut arguments = [None, None];
            arguments[position] = Some(&polygon);
            let prepared = GeometryFunction::Distance.specialize(&arguments, &budget).unwrap();
            for i in 0..=40_u32 {
                for j in 0..=40_u32 {
                    let point = Geometry::point(f64::from(i) * 0.5_f64 - 5.0_f64, f64::from(j) * 0.37_f64 - 2.0_f64, 0).unwrap().to_value();
                    let mut pair = [point.clone(), point];
                    pair[position] = polygon.clone();
                    same_result(prepared.0.call(&pair).unwrap(), GeometryFunction::Distance.call(&pair).unwrap());
                }
            }
            assert!(budget.reservations().iter().any(|(what, _)| what.contains("edge hierarchy")), "the search ran");
        }
    }
}

/// Verifies: [DESIGN] D38, D85: a polygon whose rays cross many teeth
/// builds its cell certificates once its interval trees have tested
/// 2,048 edges per edge, long before it has located eight points per
/// edge; a polygon whose rays cross few edges still waits for the points.
#[test]
fn certificates_are_built_once_ray_work_repays_them() {
    let budget = MemoryBudget::new(usize::MAX);
    let mut teeth = vec![[0.0_f64, 0.0_f64]];
    for t in 0..1_024_u32 {
        let x = f64::from(t) * 0.25_f64;
        teeth.extend([[x, 1.0_f64], [x, 9.0_f64], [x + 0.125_f64, 9.0_f64], [x + 0.125_f64, 1.0_f64]]);
    }
    teeth.extend([[256.0_f64, 1.0_f64], [256.0_f64, 0.0_f64], [0.0_f64, 0.0_f64]]);
    let comb = Geometry::of(Shape::Polygon(vec![teeth]), 0).unwrap().to_value();
    let ring = Geometry::of(Shape::Polygon(vec![dense_ring(0.0, 10.0)]), 0).unwrap().to_value();
    for (polygon, points, built) in [(comb, 6_000_u32, true), (ring, 700, false)] {
        let prepared = GeometryFunction::Intersects.specialize(&[None, Some(&polygon)], &budget).unwrap();
        for i in 0..points {
            let point = Geometry::point(f64::from(i % 997) * 0.25_f64 + 0.06_f64, 2.0_f64 + f64::from(i % 7), 0).unwrap();
            prepared.0.call(&[point.to_value(), polygon.clone()]).unwrap();
        }
        let certified = budget.reservations().iter().any(|(what, _)| what.contains("cell certificates"));
        assert_eq!(certified, built);
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

/// Verifies: [DESIGN] D85: a prepared polygon decides a comparison of a
/// point's distance to it with a constant, either operand first, by each
/// comparison operator, as SQL compares the call's value, by its cells
/// once they are built, without measuring most points; a point's decision
/// from its coordinates, as an index entry holds them, agrees; and so is
/// an intersection compared with one decided. A null, an empty point, a
/// changed operand and a line are left to the call, and a point of
/// another reference raises as the call would.
#[test]
fn prepared_comparisons_decide_as_the_measures_compare() {
    use crate::value::{CompOp, Decimal};
    let budget = MemoryBudget::new(usize::MAX);
    let mut teeth = vec![[0.0_f64, 0.0_f64]];
    for t in 0..40_u32 {
        let x = f64::from(t) * 0.25_f64;
        teeth.extend([[x, 1.0_f64], [x, 9.0_f64], [x + 0.125_f64, 9.0_f64], [x + 0.125_f64, 1.0_f64]]);
    }
    teeth.extend([[10.0_f64, 1.0_f64], [10.0_f64, 0.0_f64], [0.0_f64, 0.0_f64]]);
    let comb = Geometry::of(Shape::Polygon(vec![teeth]), 0).unwrap().to_value();
    let holed = Geometry::of(Shape::Polygon(vec![dense_ring(0.0, 10.0), dense_ring(2.0, 8.0)]), 0).unwrap().to_value();
    let radii: Vec<Value> = [0_i128, 1, 3, 10, 25, -1].into_iter().map(|tenths| Value::Exact(Decimal::new(tenths, 1_u32))).collect();
    let operators = [CompOp::Le, CompOp::Lt, CompOp::Ge, CompOp::Gt, CompOp::Eq, CompOp::Ne];
    for (polygon, other) in [(&comb, &holed), (&holed, &comb)] {
        for position in [0_usize, 1] {
            let mut arguments = [None, None];
            arguments[position] = Some(polygon);
            let prepared = GeometryFunction::Distance.specialize(&arguments, &budget).unwrap();
            for i in 0..=100_u32 {
                for j in 0..=100_u32 {
                    let point = [f64::from(i) * 0.15_f64 - 2.5_f64, f64::from(j) * 0.15_f64 - 2.5_f64];
                    let value = Geometry::point(point[0], point[1], 0_i32).unwrap().to_value();
                    let mut pair = [&value, &value];
                    pair[position] = polygon;
                    point_decisions(&prepared, (pair, point, polygon), (&radii, &operators));
                }
            }
            let reservations = budget.reservations();
            assert!(reservations.iter().any(|(what, _)| what.contains("cell certificates")), "the cells were built");
            assert!(reservations.iter().any(|(what, _)| what.contains("cell distances")), "the cell distances were counted");
            intersections_decide(&arguments, position, polygon, &budget);
            left_to_the_call(&prepared, position, (polygon, other), &radii[1]);
        }
    }
}

/// The comparisons of the call of `pair`, the point at `point` and
/// `polygon`, with each of `radii` by each of `operators`, decided as
/// the call's value compares: of the values, and of the point's
/// coordinates as an index entry holds them.
fn point_decisions(
    prepared: &crate::functions::ScalarRef,
    (pair, point, polygon): ([&Value; 2], [f64; 2], &Value),
    (radii, operators): (&[Value], &[crate::value::CompOp]),
) {
    let measured = prepared.0.call_borrowed(&pair, None).unwrap();
    for (radius, op) in radii.iter().flat_map(|radius| operators.iter().map(move |op| (radius, *op))) {
        let expected = crate::value::compare(&measured, radius).map(|ordering| op.holds(ordering));
        assert_eq!(prepared.0.call_compared(&pair, op, radius).unwrap().unwrap(), expected, "{point:?} {op:?} {radius}");
        let entry = prepared.0.point_compared((point, 0_i32), polygon, op, radius).unwrap().unwrap();
        assert_eq!(entry, expected, "{point:?} {op:?} {radius} of the entry");
    }
}

/// An intersection of each of a few points with the polygon of
/// `arguments`, compared with one, is decided as the call compares.
fn intersections_decide(arguments: &[Option<&Value>; 2], position: usize, polygon: &Value, budget: &std::sync::Arc<MemoryBudget>) {
    use crate::value::{CompOp, Decimal, compare};
    let meets = GeometryFunction::Intersects.specialize(arguments, budget).unwrap();
    let one = Value::Exact(Decimal::new(1, 0_u32));
    for text in ["POINT (0.06 5)", "POINT (0.2 5)", "POINT (5 0.5)", "POINT (5 5)", "POINT (12 12)"] {
        let value = Geometry::from_wkt(text, 0).unwrap().to_value();
        let mut pair = [&value, &value];
        pair[position] = polygon;
        let expected = compare(&meets.0.call_borrowed(&pair, None).unwrap(), &one).map(std::cmp::Ordering::is_eq);
        assert_eq!(meets.0.call_compared(&pair, CompOp::Eq, &one).unwrap().unwrap(), expected, "{text}");
    }
}

/// The comparisons `prepared`, of `polygon` at `position`, leaves to the
/// call: of a null, of `other`, another operand, of an empty point and a
/// line; and a point of another reference raises as the call would.
fn left_to_the_call(prepared: &crate::functions::ScalarRef, position: usize, (polygon, other): (&Value, &Value), radius: &Value) {
    use crate::value::CompOp;
    let point = Geometry::point(5.0_f64, 5.0_f64, 0_i32).unwrap().to_value();
    for (left, right) in [(&Value::Null, polygon), (&point, other)] {
        let pair = if position == 1 { [right, left] } else { [left, right] };
        assert!(prepared.0.call_compared(&pair, CompOp::Le, radius).is_none(), "{pair:?}");
    }
    for text in ["POINT EMPTY", "LINESTRING (1 1, 2 2)"] {
        let value = Geometry::from_wkt(text, 0).unwrap().to_value();
        let mut pair = [&value, &value];
        pair[position] = polygon;
        assert!(prepared.0.call_compared(&pair, CompOp::Le, radius).is_none(), "{text}");
    }
    let foreign = Geometry::point(5.0_f64, 5.0_f64, 4326).unwrap().to_value();
    let mut pair = [&foreign, &foreign];
    pair[position] = polygon;
    assert_eq!(prepared.0.call_compared(&pair, CompOp::Le, radius).unwrap().unwrap_err().code, Code::Data);
    let at = [5.0_f64, 5.0_f64];
    assert_eq!(prepared.0.point_compared((at, 4326_i32), polygon, CompOp::Le, radius).unwrap().unwrap_err().code, Code::Data);
    assert!(prepared.0.point_compared((at, 0_i32), other, CompOp::Le, radius).is_none());
}
