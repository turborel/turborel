//! Planar distances, with exact rational fixtures for numeric extremes.

use super::*;
use crate::functions::ScalarFunction;

/// Verifies: [EXTENSION] D82; OGC 06-103r4 §6.1.2.4: minimum XY distance
/// accounts for segment interiors, polygon interiors and holes, all basic
/// collection kinds, and degeneracies. Operand order cannot change it.
#[test]
fn distances_measure_point_sets_and_polygon_holes() {
    let polygon = "POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (4 4, 4 6, 6 6, 6 4, 4 4))";
    let cases = [
        ("POINT (0 0)", "POINT (3 4)", 5.0_f64),
        ("POINT (2 3)", "LINESTRING (0 0, 4 0)", 3.0_f64),
        ("POINT (-3 4)", "LINESTRING (0 0, 4 0)", 5.0_f64),
        ("POINT (7 4)", "LINESTRING (0 0, 4 0)", 5.0_f64),
        ("POINT (3 4)", "LINESTRING (0 0, 0 0)", 5.0_f64),
        ("LINESTRING (0 0, 4 0)", "LINESTRING (1 -2, 1 2)", 0.0_f64),
        ("LINESTRING (0 0, 4 0)", "LINESTRING (2 3, 8 3)", 3.0_f64),
        ("POINT (2 3)", polygon, 0.0_f64),
        ("POINT (5 5)", polygon, 1.0_f64),
        ("POINT (4 5)", polygon, 0.0_f64),
        ("POINT (13 14)", polygon, 5.0_f64),
        ("LINESTRING (5 5, 5 5.5)", polygon, 0.5_f64),
        ("POLYGON ((12 0, 14 0, 14 2, 12 2, 12 0))", polygon, 2.0_f64),
        ("MULTIPOINT (EMPTY, (20 20), (13 14))", polygon, 5.0_f64),
        ("MULTILINESTRING (EMPTY, (2 13, 8 13))", polygon, 3.0_f64),
        ("MULTIPOLYGON (EMPTY, ((12 0, 14 0, 14 2, 12 2, 12 0)))", polygon, 2.0_f64),
        ("GEOMETRYCOLLECTION (POINT EMPTY, GEOMETRYCOLLECTION (POINT (3 4)))", "POINT (0 0)", 5.0_f64),
    ];
    for (left, right, expected) in cases {
        let a = Geometry::from_wkt(left, 0).unwrap();
        let b = Geometry::from_wkt(right, 0).unwrap();
        assert_eq!(a.distance(&b).unwrap(), Some(expected), "{left}; {right}");
        assert_eq!(b.distance(&a).unwrap(), Some(expected), "{right}; {left}");
    }
}

/// Verifies: [EXTENSION] D82: empty sets have no distance, NULL propagates,
/// reference errors precede empty shortcuts, and only the final minimum
/// can overflow. One unrepresentable component does not poison the result.
#[test]
fn distance_null_empty_reference_and_overflow_contracts() {
    let empty = Geometry::from_wkt("GEOMETRYCOLLECTION (POINT EMPTY, LINESTRING EMPTY)", 0).unwrap();
    let origin = Geometry::point(0.0, 0.0, 0).unwrap();
    assert_eq!(empty.distance(&origin).unwrap(), None);
    assert_eq!(origin.distance(&empty).unwrap(), None);
    assert_eq!(empty.distance(&empty).unwrap(), None);
    assert_eq!(empty.distance(&empty.clone().with_srid(1)).unwrap_err().code, Code::Data);
    let function = GeometryFunction::Distance;
    assert!(function.call(&[origin.to_value(), empty.to_value()]).unwrap().is_null());
    assert!(function.call(&[Value::Null, origin.to_value()]).unwrap().is_null());
    let far = Geometry::point(f64::MAX, f64::MAX, 0).unwrap();
    let error = origin.distance(&far).unwrap_err();
    assert_eq!(error.code, Code::Data);
    assert_eq!(error.state, Some(SqlState::NUMERIC_VALUE_OUT_OF_RANGE));
    let collection = Geometry::from_wkt(&format!("MULTIPOINT (({} {}), (3 4))", f64::MAX, f64::MAX), 0).unwrap();
    assert_eq!(origin.distance(&collection).unwrap(), Some(5.0_f64));
}

/// Verifies: [DESIGN] D82: cancellation, overflow and subnormal products
/// retain finite perpendicular distances. Expected binary64 values were
/// independently rounded from exact rational squared distances using
/// 200-digit decimal square roots, not the implementation's determinant.
#[test]
fn extreme_segment_distances_match_independent_rational_oracles() {
    let cases = [
        ([1.0_f64, 1.0_f64.next_up()], [0.0_f64, 0.0_f64], [2.0_f64, 2.0_f64], 0x3ca6_a09e_667f_3bcd_u64),
        ([0.0_f64, f64::from_bits(1)], [-f64::MAX, -f64::MAX], [f64::MAX, f64::MAX], 1_u64),
        ([0.0_f64, 1.0_f64], [-f64::MAX, -f64::MAX], [f64::MAX, f64::MAX], 0x3fe6_a09e_667f_3bcd_u64),
        ([1e308_f64, 0.0_f64], [-1e308_f64, -1e308_f64], [1e308_f64, 1e308_f64], 0x7fd9_2c80_954c_51f5_u64),
        ([0.0_f64, 1e-308_f64], [-1e-308_f64, -1e-308_f64], [1e-308_f64, 1e-308_f64], 0x0005_15ab_3b15_857e_u64),
        ([0.0_f64, 1.0_f64], [0.0_f64, 0.0_f64], [1e-300_f64, 0.0_f64], 0x3ff0_0000_0000_0000_u64),
    ];
    for (p, a, b, expected) in cases {
        let actual = robust::distance::segment(p, a, b);
        assert!(actual.to_bits().abs_diff(expected) <= 2, "{p:?}, {a:?}, {b:?}: {actual:e}");
        assert_eq!(actual, robust::distance::segment(p, b, a));
    }
}

/// Verifies: [DESIGN] D82: scale does not affect endpoint/interior
/// classification across every finite binary64 exponent. Tiny segments
/// and huge differences neither collapse nor cause intermediate errors.
#[test]
fn segment_distances_survive_every_binary64_exponent() {
    for exponent in 0_u64..2047 {
        let x = f64::from_bits(exponent << 52);
        let next = x.next_up();
        let actual = robust::distance::segment([x, next], [x, x], [next, x]);
        assert_eq!(actual, next - x, "exponent {exponent}");
    }
}

/// Verifies: [DESIGN] D82: ordinary segment distances agree with an
/// independent integer projection oracle in all endpoint regions. The
/// error allowance concerns approximate measures, not topology decisions.
#[test]
fn segment_distances_agree_with_integer_projection_oracle() {
    let mut seed = 73_u32;
    for _ in 0_u32..10_000 {
        let mut next = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            i32::try_from(seed >> 19_u32).unwrap() - 4096_i32
        };
        let [px, py, ax, ay, bx, by] = [next(), next(), next(), next(), next(), next()];
        let (dx, dy, ox, oy) = (bx - ax, by - ay, px - ax, py - ay);
        let projection = ox * dx + oy * dy;
        let length = dx * dx + dy * dy;
        let expected = if projection <= 0_i32 {
            f64::from(ox).hypot(f64::from(oy))
        } else if projection >= length {
            f64::from(px - bx).hypot(f64::from(py - by))
        } else {
            f64::from((ox * dy - oy * dx).abs()) / f64::from(length).sqrt()
        };
        let actual = robust::distance::segment([px, py].map(f64::from), [ax, ay].map(f64::from), [bx, by].map(f64::from));
        assert!((actual - expected).abs() <= expected * 8.0_f64 * f64::EPSILON, "actual {actual:e}, expected {expected:e}");
    }
}
