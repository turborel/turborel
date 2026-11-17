//! Interval pruning retains the ordinary ring decisions and obeys the
//! same representation and memory bounds as its prepared operand.

use super::*;
use crate::geometry::relate;
use crate::geometry::tests::common::{dense_ring as ring, subdivided_ring};

fn with_hole(reversed: bool) -> Shape {
    let mut shell = ring(0.0, 10.0);
    let mut hole = ring(2.0, 8.0);
    if reversed {
        shell.reverse();
        hole.reverse();
    }
    Shape::Polygon(vec![shell, hole])
}

/// Verifies: [DESIGN] D85: interval pruning preserves the exact shell,
/// hole and boundary decisions in either ring direction. Empty polygon
/// members, disconnected components and outside points keep their meaning.
#[test]
fn polygon_interval_locations_match_all_ring_decisions() {
    let budget = MemoryBudget::new(usize::MAX);
    for reversed in [false, true] {
        for shape in [
            with_hole(reversed),
            Shape::Multi(MULTIPOLYGON, vec![Shape::Polygon(Vec::new()), with_hole(reversed), Shape::Polygon(vec![ring(12.0, 14.0)])]),
            Shape::Polygon(vec![subdivided_ring(&[
                [0.0_f64, 0.0_f64],
                [10.0_f64, 0.0_f64],
                [10.0_f64, 3.0_f64],
                [3.0_f64, 3.0_f64],
                [3.0_f64, 10.0_f64],
                [0.0_f64, 10.0_f64],
                [0.0_f64, 0.0_f64],
            ])]),
        ] {
            let locator = Locator::new(&shape, &budget).unwrap();
            for x in -2_i32..31_i32 {
                for y in -2_i32..31_i32 {
                    let point = [f64::from(x) / 2.0_f64, f64::from(y) / 2.0_f64];
                    assert!(locator.locate(point).unwrap() == relate::locate(point, &shape), "point {point:?}, reversed {reversed}");
                }
            }
        }
    }
    assert_eq!(budget.reserved(), 0);
}

/// Verifies: [DESIGN] D85: closed interval bounds retain vertices and
/// horizontal edges at subnormal and extreme finite ordinates. A value
/// immediately outside either bound is distinguished from its boundary;
/// the Y tree does not calculate an overflowing or rounded midpoint.
/// Inside, below and above points isolate both interval tests (MC/DC).
#[test]
fn polygon_interval_bounds_keep_extreme_ordinates() {
    let budget = MemoryBudget::new(usize::MAX);
    for scale in [f64::from_bits(1), f64::MIN_POSITIVE, 1.0_f64, f64::MAX] {
        let shape = Shape::Polygon(vec![ring(-scale, scale)]);
        assert!(shape.bounds().is_ok());
        let locator = Locator::new(&shape, &budget).unwrap();
        for point in [
            [0.0_f64, 0.0_f64],
            [scale, scale],
            [0.0_f64, scale],
            [0.0_f64, -scale],
            [0.0_f64, scale.next_down()],
            [0.0_f64, (-scale).next_up()],
        ] {
            assert!(locator.locate(point).unwrap() == relate::locate(point, &shape), "point {point:?}");
        }
        if scale < f64::MAX {
            for y in [scale.next_up(), (-scale).next_down()] {
                assert!(locator.locate([0.0_f64, y]).unwrap() == Location::Exterior);
            }
        }
    }
}

/// Verifies: [DESIGN] D38, D85: the optional index reserves every owned
/// vector's capacity and releases it on success or partial construction
/// failure. Unsupported kinds and empty values allocate no index.
#[test]
fn polygon_interval_indexes_reserve_and_release_their_storage() {
    let budget = MemoryBudget::new(usize::MAX);
    let shape = with_hole(false);
    let locator = Locator::new(&shape, &budget).unwrap();
    let bytes = budget.reserved();
    assert!(bytes >= 128 * size_of::<Edge>());
    drop(locator);
    assert_eq!(budget.reserved(), 0);
    for limit in [0, size_of::<Locator>() - 1, bytes - 1] {
        budget.set_limit(limit);
        assert!(Locator::new(&shape, &budget).is_none());
        assert_eq!(budget.reserved(), 0);
    }
    budget.set_limit(bytes);
    drop(Locator::new(&shape, &budget).unwrap());
    let small = crate::geometry::Geometry::rectangle([0.0_f64, 0.0_f64], [1.0_f64, 1.0_f64], 0).unwrap().shape;
    for unsupported in [
        Shape::Point(Some([0.0_f64, 0.0_f64])),
        Shape::Line(ring(0.0, 1.0)),
        Shape::Polygon(Vec::new()),
        small.clone(),
        Shape::Multi(MULTIPOLYGON, vec![small; MIN_EDGES]),
        Shape::Multi(crate::geometry::shape::COLLECTION, vec![with_hole(false)]),
        Shape::Multi(MULTIPOLYGON, vec![Shape::Point(None)]),
    ] {
        assert!(Locator::new(&unsupported, &budget).is_none());
    }
    assert_eq!(budget.reserved(), 0);
}

/// Verifies: [DESIGN] R-CF-6, D85: the median tree accommodates the full
/// coordinate limit. Both construction and querying refuse a nonempty
/// subtree beyond that depth; an empty child needs no recursive frame.
#[test]
fn polygon_interval_depth_is_bounded_by_representation_size() {
    let mut edges: Vec<_> = (0..MAX_PARTS)
        .map(|n| {
            let y = f64::from(u32::try_from(n).unwrap());
            Edge { start: [0.0_f64, y], end: [1.0_f64, y], maximum: y }
        })
        .collect();
    maxima(&mut edges, 0).unwrap();
    assert!(in_ring([0.5_f64, 50_000.0_f64], &edges).unwrap() == Location::Boundary);
    assert!(in_ring([0.5_f64, 50_000.5_f64], &edges).unwrap() == Location::Exterior);
    assert_eq!(maxima(&mut edges, LEVELS).unwrap_err().code, Code::Api);
    assert_eq!(visit(&edges, &mut Ray::new([0.0_f64, 0.0_f64]), LEVELS).unwrap_err().code, Code::Api);
    assert_eq!(maxima(&mut [], LEVELS).unwrap().to_bits(), f64::NEG_INFINITY.to_bits());
    assert!(!visit(&[], &mut Ray::new([0.0_f64, 0.0_f64]), LEVELS).unwrap());
}
