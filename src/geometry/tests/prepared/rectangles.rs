//! Exact rectangular point location preserves the scalar contract.

use super::*;

/// Verifies: [DESIGN] D15, D78: prepared rectangles preserve intersection,
/// disjointness and distance bits at corners, edges, interior and exterior
/// points, including adjacent binary64 values and extreme coordinates.
/// Both operand orders, NULL, EMPTY and changed SRIDs retain their results
/// or errors; non-point operands continue through ordinary relationships.
#[test]
fn prepared_rectangles_match_general_relationships_at_every_boundary() {
    let budget = MemoryBudget::new(usize::MAX);
    for (low, high) in [(0.0_f64, 1.0_f64), (0.0_f64, f64::from_bits(1)), (-f64::MAX, f64::MAX)] {
        let fixed = Geometry::rectangle([low, low], [high, high], 0).unwrap().to_value();
        let coordinates = [low.next_down(), low, low.next_up(), high.next_down(), high, high.next_up()];
        let mut others = vec![Value::Null, Geometry::point(low, high, 4326).unwrap().to_value()];
        for &x in &coordinates {
            for &y in &coordinates {
                if x.is_finite() && y.is_finite() {
                    others.push(Geometry::point(x, y, 0).unwrap().to_value());
                }
            }
        }
        for text in ["POINT EMPTY", "LINESTRING (0 0, 2 2)", "MULTIPOINT ((0 0), (2 2))"] {
            others.push(Geometry::from_wkt(text, 0).unwrap().to_value());
        }
        check(&fixed, &others, &budget);
    }
    assert_eq!(budget.reserved(), 0);
}

fn check(fixed: &Value, others: &[Value], budget: &std::sync::Arc<MemoryBudget>) {
    for function in [GeometryFunction::Intersects, GeometryFunction::Disjoint, GeometryFunction::Distance] {
        for position in [0_usize, 1] {
            let mut known = [None, None];
            known[position] = Some(fixed);
            let prepared = function.specialize(&known, budget).unwrap();
            for other in others {
                let mut args = [other.clone(), other.clone()];
                args[position] = fixed.clone();
                match function.call(&args) {
                    Ok(expected) => same_result(prepared.0.call(&args).unwrap(), expected),
                    Err(expected) => assert_eq!(prepared.0.call(&args).unwrap_err().code, expected.code),
                }
            }
        }
    }
}
