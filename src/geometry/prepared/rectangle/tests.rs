//! Rectangle recognition is exact and independent of ring orientation.

use super::*;
use crate::geometry::Geometry;

/// Verifies: [DESIGN] D82: all four starting corners and both ring
/// directions describe the same rectangle, even at subnormal and extreme
/// coordinates. Recognition requires no subtraction or area calculation.
#[test]
fn rectangles_accept_each_start_and_direction_without_area_arithmetic() {
    for (lower, upper) in [
        ([0.0_f64, 0.0_f64], [1.0_f64, 1.0_f64]),
        ([0.0_f64, 0.0_f64], [f64::from_bits(1), f64::from_bits(1)]),
        ([-f64::MAX, -f64::MAX], [f64::MAX, f64::MAX]),
    ] {
        let g = Geometry::rectangle(lower, upper, 0).unwrap();
        let Shape::Polygon(mut rings) = g.shape else { panic!("rectangle") };
        let mut corners = rings.pop().unwrap();
        corners.pop();
        for _ in 0_usize..4 {
            for reverse in [false, true] {
                let mut ring = corners.clone();
                if reverse {
                    ring.reverse();
                }
                ring.push(ring[0]);
                assert!(recognizes(&Shape::Polygon(vec![ring])));
            }
            corners.rotate_left(1);
        }
    }
}

/// Verifies: [DESIGN] D82: bounds alone cannot recognize a rectangle.
/// Each required shape, ring count, corner count, closure, nonzero extent
/// and adjacent-corner relationship is varied independently.
#[test]
fn other_shapes_holes_diagonals_and_collapsed_rings_are_declined() {
    for text in [
        "POINT (0 0)",
        "POLYGON EMPTY",
        "LINESTRING (0 0, 1 1)",
        "POLYGON ((0 0, 2 0, 2 2, 0 2, 0 0), (0.5 0.5, 1 0.5, 1 1, 0.5 0.5))",
        "POLYGON ((0 0, 2 0, 2 2, 0 0))",
        "POLYGON ((0 0, 2 2, 2 0, 0 2, 0 0))",
        "POLYGON ((0 0, 2 0, 2 2, 1 2, 0 0))",
        "POLYGON ((0 0, 1 0, 2 2, 0 2, 0 0))",
        "POLYGON ((0 0, 0 0, 0 2, 0 2, 0 0))",
        "POLYGON ((0 0, 2 0, 2 0, 0 0, 0 0))",
    ] {
        assert!(!recognizes(&Geometry::from_wkt(text, 0).unwrap().shape), "{text}");
    }
    let open: Vec<Point> = vec![[0.0, 0.0], [2.0, 0.0], [2.0, 2.0], [0.0, 2.0], [1.0, 1.0]];
    assert!(!recognizes(&Shape::Polygon(vec![open])));
}
