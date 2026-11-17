//! Borrowed geometry calls retain ordinary and prepared scalar semantics.

use super::*;

fn same_call(function: &dyn ScalarFunction, args: &[Value]) {
    let refs: Vec<_> = args.iter().collect();
    match function.call(args) {
        Ok(expected) => same_result(function.call_borrowed(&refs, None).unwrap(), expected),
        Err(expected) => assert_eq!(function.call_borrowed(&refs, None).unwrap_err().code, expected.code),
    }
}

/// Verifies: [DESIGN] D15, D85: borrowed geometry relationships preserve
/// NULL, EMPTY, boundary and interior results, SRID and malformed-value
/// errors, arity checks and changed prepared constants in either position.
/// The other geometry functions retain their ordinary call contract.
#[test]
fn borrowed_geometry_calls_keep_results_and_errors() {
    let budget = MemoryBudget::new(usize::MAX);
    let rectangle = Geometry::rectangle([0.0_f64, 0.0_f64], [10.0_f64, 10.0_f64], 0).unwrap().to_value();
    let point = Geometry::point(5.0, 5.0, 0).unwrap().to_value();
    let values = [
        Value::Null,
        point.clone(),
        Geometry::point(0.0, 0.0, 0).unwrap().to_value(),
        Geometry::point(5.0, 5.0, 4326).unwrap().to_value(),
        Geometry::from_wkt("POINT EMPTY", 0).unwrap().to_value(),
        Geometry::from_wkt("LINESTRING (0 0, 20 20)", 0).unwrap().to_value(),
        Value::Char("not geometry".into()),
    ];
    for function in [GeometryFunction::Intersects, GeometryFunction::Disjoint, GeometryFunction::Distance] {
        for position in [0_usize, 1] {
            let mut known = [None, None];
            known[position] = Some(&rectangle);
            let prepared = function.specialize(&known, &budget).unwrap();
            for fixed in [&rectangle, &point] {
                for other in &values {
                    let mut pair = [other.clone(), other.clone()];
                    pair[position] = fixed.clone();
                    same_call(&function, &pair);
                    same_call(&*prepared.0, &pair);
                }
            }
            for args in [&[][..], &[Value::Null][..], &[point.clone(), Value::Null, Value::Null][..]] {
                same_call(&function, args);
                same_call(&*prepared.0, args);
            }
        }
    }
    for function in FUNCTIONS {
        for args in [&[][..], &[point.clone()][..], &[Value::Null, Value::Null][..], &[Value::Null, Value::Null, Value::Null][..]] {
            same_call(&function, args);
        }
    }
}
