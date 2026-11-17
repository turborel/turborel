//! Computed distances and envelope bounds used by geometry pruning.

use super::*;

/// Verifies: [DESIGN] D85: geometry distances never fall below the
/// computed envelope bound. Boundary fixtures and a deterministic sample
/// spanning binary64 magnitudes inside MAX/8 retain finite point/segment
/// distances. Swapping operands preserves the result; reversing a line's
/// endpoints stays within two ULPs on this fixture sample.
#[test]
fn envelope_bounds_respect_finite_geometry_distances() {
    let limit = f64::MAX / 8.0_f64;
    let tiny = f64::from_bits(1);
    let cases = [
        ([limit, limit], [-limit, -limit], [-limit, -limit]),
        ([limit, -limit], [-limit, -limit], [limit, limit]),
        ([limit, 0.0_f64], [-limit, -limit], [limit, limit]),
        ([0.0_f64, tiny], [-limit, -limit], [limit, limit]),
        ([tiny, 0.0_f64], [0.0_f64, tiny], [0.0_f64, tiny]),
    ];
    for (point, start, end) in cases {
        finite(point, start, end);
    }
    let mut seed = 73_u64;
    for _ in 0_u32..1024 {
        let point = [coordinate(&mut seed), coordinate(&mut seed)];
        let start = [coordinate(&mut seed), coordinate(&mut seed)];
        let end = [coordinate(&mut seed), coordinate(&mut seed)];
        finite(point, start, end);
    }
}

fn coordinate(seed: &mut u64) -> f64 {
    *seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
    let magnitude = f64::from_bits(*seed & 0x7fbf_ffff_ffff_ffff);
    if *seed >> 63 == 0 { magnitude } else { -magnitude }
}

fn finite(point: shape::Point, start: shape::Point, end: shape::Point) {
    let point = Geometry::of(Shape::Point(Some(point)), 0).unwrap();
    let line = Geometry::of(Shape::Line(vec![start, end]), 0).unwrap();
    let reverse = Geometry::of(Shape::Line(vec![end, start]), 0).unwrap();
    let value = point.distance(&line).unwrap().unwrap();
    assert!(value.is_finite());
    assert!(value >= point.bounds().unwrap().distance_lower_bound(line.bounds().unwrap()));
    assert_eq!(line.distance(&point).unwrap(), Some(value));
    let reversed = point.distance(&reverse).unwrap().unwrap();
    assert!(reversed.to_bits().abs_diff(value.to_bits()) <= 2, "{value:e}, {reversed:e}");
}
