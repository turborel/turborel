//! Segment meetings, rounded crossings and hot pixels.

use super::super::dyadic::Dyadic;
use super::super::tests::Seeded;
use super::*;

/// Verifies: [DESIGN] each kind of meeting of two segments, collinear
/// and degenerate ones included.
#[test]
fn meetings() {
    let p = |x: f64, y: f64| [x, y];
    assert_eq!(meet(p(0.0_f64, 0.0_f64), p(2.0_f64, 2.0_f64), p(0.0_f64, 2.0_f64), p(2.0_f64, 0.0_f64)), Meeting::Cross);
    assert_eq!(
        meet(p(0.0_f64, 0.0_f64), p(2.0_f64, 0.0_f64), p(1.0_f64, 0.0_f64), p(1.0_f64, 1.0_f64)),
        Meeting::Touch(p(1.0_f64, 0.0_f64))
    );
    assert_eq!(
        meet(p(0.0_f64, 0.0_f64), p(2.0_f64, 0.0_f64), p(2.0_f64, 0.0_f64), p(3.0_f64, 1.0_f64)),
        Meeting::Touch(p(2.0_f64, 0.0_f64))
    );
    assert_eq!(
        meet(p(0.0_f64, 0.0_f64), p(2.0_f64, 0.0_f64), p(1.0_f64, 0.0_f64), p(3.0_f64, 0.0_f64)),
        Meeting::Overlap(p(1.0_f64, 0.0_f64), p(2.0_f64, 0.0_f64))
    );
    assert_eq!(
        meet(p(3.0_f64, 0.0_f64), p(0.0_f64, 0.0_f64), p(2.0_f64, 0.0_f64), p(1.0_f64, 0.0_f64)),
        Meeting::Overlap(p(1.0_f64, 0.0_f64), p(2.0_f64, 0.0_f64))
    );
    assert_eq!(
        meet(p(0.0_f64, 0.0_f64), p(1.0_f64, 0.0_f64), p(1.0_f64, 0.0_f64), p(2.0_f64, 0.0_f64)),
        Meeting::Touch(p(1.0_f64, 0.0_f64))
    );
    assert_eq!(meet(p(0.0_f64, 0.0_f64), p(1.0_f64, 0.0_f64), p(2.0_f64, 0.0_f64), p(3.0_f64, 0.0_f64)), Meeting::Apart);
    assert_eq!(meet(p(0.0_f64, 0.0_f64), p(1.0_f64, 1.0_f64), p(1.0_f64, 0.0_f64), p(2.0_f64, 1.0_f64)), Meeting::Apart);
    assert_eq!(meet(p(0.0_f64, 0.0_f64), p(4.0_f64, 0.0_f64), p(2.0_f64, 1.0_f64), p(3.0_f64, 0.5_f64)), Meeting::Apart);
    assert!(same(p(0.0_f64, -0.0_f64), p(-0.0_f64, 0.0_f64)));
    assert!(on_segment(p(1.0_f64, 1.0_f64), p(0.0_f64, 0.0_f64), p(2.0_f64, 2.0_f64)));
    assert!(!on_segment(p(3.0_f64, 3.0_f64), p(0.0_f64, 0.0_f64), p(2.0_f64, 2.0_f64)));
}

/// Verifies: [DESIGN] a crossing is the nearest binary64 position to the
/// exact one. Thirds and a tiny-angle crossing are checked against the
/// exact rational value through its pixel.
#[test]
fn crossings_round_to_nearest() {
    assert_eq!(crossing([0.0_f64, 0.0_f64], [3.0_f64, 1.0_f64], [0.0_f64, 1.0_f64], [3.0_f64, 0.0_f64]), [1.5_f64, 0.5_f64]);
    assert_eq!(crossing([0.0_f64, 0.0_f64], [1.0_f64, 1.0_f64], [0.0_f64, 1.0_f64], [3.0_f64, 0.0_f64]), [0.75_f64, 0.75_f64]);
    assert_eq!(crossing([0.0_f64, 0.0_f64], [10.0_f64, 1.0_f64], [0.0_f64, 1.0_f64], [10.0_f64, 0.0_f64]), [5.0_f64, 0.5_f64]);
    let mut random = Seeded(3);
    for _ in 0_i32..2_000_i32 {
        let [a, b, c, d] = [0_i32; 4].map(|_| random.point(1_000));
        if meet(a, b, c, d) != Meeting::Cross {
            continue;
        }
        let point = crossing(a, b, c, d);
        assert!(meets_pixel(a, b, point) && meets_pixel(c, d, point), "{a:?} {b:?} {c:?} {d:?} {point:?}");
    }
}

/// Verifies: [DESIGN] hot pixels: a segment meets the pixel of a
/// position within half a unit in the last place and not one a whole
/// unit away; pixel envelopes enclose their pixels.
#[test]
fn hot_pixels() {
    let centre = [1.0_f64, 1.0_f64];
    assert!(meets_pixel([0.0_f64, 0.0_f64], [2.0_f64, 2.0_f64], centre));
    let (up, down) = (1.0_f64.next_up(), 1.0_f64.next_down());
    assert!(meets_pixel([0.0_f64, up], [2.0_f64, down], centre));
    assert!(!meets_pixel([0.0_f64, up], [2.0_f64, up], centre));
    assert!(!meets_pixel([0.0_f64, up.next_up()], [2.0_f64, down], [0.0_f64, 1.0_f64]));
    assert!(!meets_pixel([0.0_f64, 0.0_f64], [0.5_f64, 0.5_f64], centre));
    assert!(meets_pixel([1.0_f64, 1.0_f64], [1.0_f64, 1.0_f64], centre));
    let (low, high) = pixel(centre);
    assert!(low[0] < 1.0_f64 && high[1] > 1.0_f64);
}

/// Whether `value` is the nearest binary64 value to `numerator /
/// denominator`, the quotient lying between the midpoints to its
/// neighbours.
fn nearest_to(value: f64, numerator: &Dyadic, denominator: &Dyadic) -> bool {
    let (numerator, denominator) = if denominator.sign() == Ordering::Less {
        let zero = Dyadic::of(0.0_f64);
        (zero.minus(numerator), zero.minus(denominator))
    } else {
        (numerator.clone(), denominator.clone())
    };
    let middle = |other: f64| Dyadic::of(value).plus(&Dyadic::of(other)).halved().times(&denominator);
    numerator.minus(&middle(value.next_up())).sign() != Ordering::Greater
        && numerator.minus(&middle(value.next_down())).sign() != Ordering::Less
}

/// Verifies: [DESIGN] crossings of segments with full 53-bit coordinates
/// round to the nearest position on each axis, by the exact rational
/// value `a + (b - a) t`.
#[test]
fn full_precision_crossings_are_nearest() {
    let mut random = Seeded(29);
    let mut fine = || {
        let high = f64::from(u32::try_from(random.below(1 << 26)).unwrap());
        let low = f64::from(u32::try_from(random.below(1 << 26)).unwrap());
        high / 67_108_864.0_f64 + low / 4_503_599_627_370_496.0_f64
    };
    let mut checked = 0_u32;
    for _ in 0_i32..4_000_i32 {
        let [a, b, c, d] = [0_i32; 4].map(|_| [fine(), fine()]);
        if meet(a, b, c, d) != Meeting::Cross {
            continue;
        }
        let [ax, ay] = a.map(Dyadic::of);
        let ([bx, by], [cx, cy], [dx, dy]) = (b.map(Dyadic::of), c.map(Dyadic::of), d.map(Dyadic::of));
        let (run, rise, other_run, other_rise) = (bx.minus(&ax), by.minus(&ay), dx.minus(&cx), dy.minus(&cy));
        let along = cx.minus(&ax).times(&other_rise).minus(&cy.minus(&ay).times(&other_run));
        let denominator = run.times(&other_rise).minus(&rise.times(&other_run));
        let [across, up] = crossing(a, b, c, d);
        assert!(nearest_to(across, &ax.times(&denominator).plus(&run.times(&along)), &denominator), "{a:?} {b:?} {c:?} {d:?}");
        assert!(nearest_to(up, &ay.times(&denominator).plus(&rise.times(&along)), &denominator), "{a:?} {b:?} {c:?} {d:?}");
        checked = checked.checked_add(1).unwrap();
    }
    assert!(checked > 100, "{checked}");
}
