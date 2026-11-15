//! Requirements for conservative, finite spatial bounds.

use super::*;

/// Verifies: [DESIGN] Spatial coordinates are finite and each lower bound
/// is at most its upper bound. Reversed X and Y independently exercise
/// the two conditions; equality is valid on either axis.
#[test]
fn coordinates_are_finite_and_ordered() {
    for lower in [[2.0_f64, 0.0_f64], [0.0_f64, 2.0_f64]] {
        assert_eq!(Bounds::new(lower, [1.0_f64, 1.0_f64]).unwrap_err().code, Code::Api);
    }
    for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        for axis in 0..DIMENSIONS {
            let mut lower = [0.0_f64; DIMENSIONS];
            lower[axis] = invalid;
            assert_eq!(Bounds::new(lower, [1.0_f64, 1.0_f64]).unwrap_err().code, Code::Api);
            let mut upper = [1.0_f64; DIMENSIONS];
            upper[axis] = invalid;
            assert_eq!(Bounds::new([0.0_f64, 0.0_f64], upper).unwrap_err().code, Code::Api);
        }
    }
    assert!(Bounds::new([1.0_f64, 0.0_f64], [1.0_f64, 1.0_f64]).is_ok());
    assert!(Bounds::new([0.0_f64, 1.0_f64], [1.0_f64, 1.0_f64]).is_ok());
}

/// Verifies: [DESIGN] Rectangles intersect at their closed boundaries,
/// including points and lines. Moving a point beyond each of the four
/// sides independently makes one intersection condition false (MC/DC).
#[test]
fn touching_points_and_edges_intersect() {
    let square = Bounds::new([0.0_f64, 0.0_f64], [2.0_f64, 2.0_f64]).unwrap();
    for point in [[0.0_f64, 0.0_f64], [0.0_f64, 2.0_f64], [2.0_f64, 0.0_f64], [2.0_f64, 2.0_f64], [1.0_f64, 1.0_f64]] {
        let point = Bounds::point(point).unwrap();
        assert!(square.intersects(point));
        assert!(point.intersects(square));
    }
    for point in [[-1.0_f64, 1.0_f64], [3.0_f64, 1.0_f64], [1.0_f64, -1.0_f64], [1.0_f64, 3.0_f64]] {
        let point = Bounds::point(point).unwrap();
        assert!(!square.intersects(point));
        assert!(!point.intersects(square));
        assert!(square.intersection(point).is_none());
    }
    let edge = Bounds::new([2.0_f64, 0.0_f64], [2.0_f64, 2.0_f64]).unwrap();
    assert_eq!(square.intersection(edge), Some(edge));
}

/// Verifies: [DESIGN] Containment includes the boundary; crossing each
/// side independently makes one of its four conditions false (MC/DC).
/// Union encloses both inputs and intersection is inside both.
#[test]
fn unions_enclose_and_intersections_are_contained() {
    let square = Bounds::new([0.0_f64, 0.0_f64], [2.0_f64, 2.0_f64]).unwrap();
    assert!(square.contains(square));
    assert!(square.contains(Bounds::point([0.0_f64, 2.0_f64]).unwrap()));
    for (lower, upper) in [
        ([-1.0_f64, 0.0_f64], [1.0_f64, 1.0_f64]),
        ([1.0_f64, 0.0_f64], [3.0_f64, 1.0_f64]),
        ([0.0_f64, -1.0_f64], [1.0_f64, 1.0_f64]),
        ([0.0_f64, 1.0_f64], [1.0_f64, 3.0_f64]),
    ] {
        let other = Bounds::new(lower, upper).unwrap();
        assert!(!square.contains(other));
        let union = square.union(other);
        assert!(union.contains(square));
        assert!(union.contains(other));
        let intersection = square.intersection(other).unwrap();
        assert!(square.contains(intersection));
        assert!(other.contains(intersection));
    }
}

/// Verifies: [DESIGN] Signed zero has a single representation in bounds,
/// preventing two entries for the same SQL numeric key.
#[test]
fn signed_zero_is_canonical() {
    let bounds = Bounds::new([-0.0_f64, 0.0_f64], [0.0_f64, -0.0_f64]).unwrap();
    assert_eq!(bounds.lower().map(f64::to_bits), [0, 0]);
    assert_eq!(bounds.upper().map(f64::to_bits), [0, 0]);
    assert_eq!(bounds, Bounds::point([0.0_f64, 0.0_f64]).unwrap());
}

/// Verifies: [DESIGN] Bounds are identical only if both lower and upper
/// coordinates match. Changing any one coordinate changes their identity.
#[test]
fn bounds_identity_checks_every_coordinate() {
    let square = Bounds::new([0.0_f64, 0.0_f64], [2.0_f64, 2.0_f64]).unwrap();
    for (lower, upper) in [
        ([1.0_f64, 0.0_f64], [2.0_f64, 2.0_f64]),
        ([0.0_f64, 1.0_f64], [2.0_f64, 2.0_f64]),
        ([0.0_f64, 0.0_f64], [3.0_f64, 2.0_f64]),
        ([0.0_f64, 0.0_f64], [2.0_f64, 3.0_f64]),
    ] {
        assert_ne!(square, Bounds::new(lower, upper).unwrap());
    }
    assert_eq!(square, square);
}

/// Verifies: [DESIGN] Distances use both axes; overlapping rectangles
/// have distance zero. The search bound never exceeds the exact 3-4-5
/// triangle distance and stays conservative through underflow/overflow.
#[test]
fn distances_bound_the_geometry_at_extreme_scales() {
    let origin = Bounds::point([0.0_f64, 0.0_f64]).unwrap();
    let point = Bounds::point([3.0_f64, 4.0_f64]).unwrap();
    assert_eq!(origin.distance(point).to_bits(), 5.0_f64.to_bits());
    assert!(origin.distance_lower_bound(point) <= 5.0_f64);
    assert!(origin.distance_lower_bound(point) > 4.0_f64);
    assert_eq!(origin.distance_lower_bound(origin).to_bits(), 0);
    let tiny = Bounds::point([f64::from_bits(1), f64::from_bits(1)]).unwrap();
    assert!(origin.distance_lower_bound(tiny) <= origin.distance(tiny));
    let far = Bounds::point([f64::MAX, f64::MAX]).unwrap();
    let opposite = Bounds::point([-f64::MAX, -f64::MAX]).unwrap();
    assert!(far.distance(opposite).is_infinite());
    assert!(far.distance_lower_bound(opposite).is_finite());
    assert!(far.distance_lower_bound(opposite) > 0.0_f64);
}

/// Verifies: [DESIGN] Enlarging a node cannot increase its search lower
/// bound: every sampled descendant remains reachable by distance pruning.
#[test]
fn enclosing_bounds_do_not_prune_nearer_children() {
    let query = Bounds::point([-3.0_f64, 11.0_f64]).unwrap();
    for x in -20_i32..20_i32 {
        for y in -20_i32..20_i32 {
            let point = Bounds::point([f64::from(x), f64::from(y)]).unwrap();
            let envelope = point.union(Bounds::point([50.0_f64, -50.0_f64]).unwrap());
            assert!(envelope.distance_lower_bound(query) <= point.distance(query));
        }
    }
}
