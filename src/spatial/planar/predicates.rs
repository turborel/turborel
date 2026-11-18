//! \[DESIGN\] Exact geometric predicates on finite binary64 coordinates:
//! the planar orientation and incircle tests here, the spatial
//! orientation and insphere tests in `predicates3d.rs`. Each is decided
//! in three stages. A floating evaluation with Shewchuk's forward error
//! bound settles the common case; outward-rounded intervals settle most
//! of the rest; exact dyadic arithmetic settles everything else. No
//! stage uses a geometric tolerance, so the answers are the exact signs.

use std::cmp::Ordering;

use super::Point;
use super::dyadic::Dyadic;
use super::interval::{Arithmetic, Interval};

/// Shewchuk, "Adaptive Precision Floating-Point Arithmetic and Fast
/// Robust Geometric Predicates" (1997), `exactinit`'s error bounds for
/// binary64 unit roundoff ε = 2^-53, with `ccwerrboundA` = (3 + 16ε)ε.
const ORIENT_BOUND: f64 = 3.330_669_073_875_471_6e-16;
/// `iccerrboundA` = (10 + 96ε)ε.
const INCIRCLE_BOUND: f64 = 1.110_223_024_625_157_7e-15;
/// The filter's analysis assumes no product underflows or overflows.
/// Every nonzero difference within [2^-200, 2^200] keeps products of up
/// to five of them, and their sums, inside binary64's normal range.
pub(super) const TAME_LOW: f64 = 6.223_015_277_861_142e-61;
pub(super) const TAME_HIGH: f64 = 1.606_938_044_258_990_3e60;

/// Whether every value is zero or of a magnitude the filter's analysis
/// covers.
pub(super) fn tame(values: &[f64]) -> bool {
    values.iter().all(|value| {
        let magnitude = value.abs();
        magnitude <= TAME_HIGH && (magnitude >= TAME_LOW || magnitude <= 0.0_f64)
    })
}

/// A sign proven by a floating value and its error bound.
pub(super) fn proven(value: f64, bound: f64) -> Option<Ordering> {
    if value > bound {
        Some(Ordering::Greater)
    } else if value < -bound {
        Some(Ordering::Less)
    } else {
        None
    }
}

/// The orientation of `c` relative to the directed line from `a` to
/// `b`, greater when `a`, `b`, `c` turn counterclockwise, less when
/// clockwise, equal when they are collinear.
pub fn orient2d(a: Point, b: Point, c: Point) -> Ordering {
    filtered_orient(a, b, c)
        .or_else(|| orient_determinant::<Interval>(a, b, c).sign())
        .unwrap_or_else(|| orient_determinant::<Dyadic>(a, b, c).sign())
}

fn filtered_orient([ax, ay]: Point, [bx, by]: Point, [cx, cy]: Point) -> Option<Ordering> {
    let differences = [ax - cx, by - cy, ay - cy, bx - cx];
    if !tame(&differences) {
        return None;
    }
    let [acx, bcy, acy, bcx] = differences;
    let left = acx * bcy;
    let right = acy * bcx;
    proven(left - right, ORIENT_BOUND * (left.abs() + right.abs()))
}

fn orient_determinant<T: Arithmetic>([ax, ay]: Point, [bx, by]: Point, [cx, cy]: Point) -> T {
    let left = T::difference(ax, cx).times(&T::difference(by, cy));
    let right = T::difference(ay, cy).times(&T::difference(bx, cx));
    left.minus(&right)
}

/// Whether `d` lies inside the circle through `a`, `b` and `c`, greater
/// inside, less outside, equal on it, when `a`, `b`, `c` turn
/// counterclockwise; the signs reverse when they turn clockwise.
pub fn incircle(a: Point, b: Point, c: Point, d: Point) -> Ordering {
    filtered_incircle(a, b, c, d)
        .or_else(|| incircle_determinant::<Interval>(a, b, c, d).sign())
        .unwrap_or_else(|| incircle_determinant::<Dyadic>(a, b, c, d).sign())
}

fn filtered_incircle([ax, ay]: Point, [bx, by]: Point, [cx, cy]: Point, [dx, dy]: Point) -> Option<Ordering> {
    let differences = [ax - dx, ay - dy, bx - dx, by - dy, cx - dx, cy - dy];
    if !tame(&differences) {
        return None;
    }
    let [adx, ady, bdx, bdy, cdx, cdy] = differences;
    let lifts = [adx * adx + ady * ady, bdx * bdx + bdy * bdy, cdx * cdx + cdy * cdy];
    let minors = [(bdx * cdy, cdx * bdy), (cdx * ady, adx * cdy), (adx * bdy, bdx * ady)];
    let (mut determinant, mut permanent) = (0.0_f64, 0.0_f64);
    for (lift, (plus, minus)) in lifts.iter().zip(minors) {
        determinant += lift * (plus - minus);
        permanent += (plus.abs() + minus.abs()) * lift;
    }
    proven(determinant, INCIRCLE_BOUND * permanent)
}

fn incircle_determinant<T: Arithmetic>([ax, ay]: Point, [bx, by]: Point, [cx, cy]: Point, [dx, dy]: Point) -> T {
    let (adx, ady) = (T::difference(ax, dx), T::difference(ay, dy));
    let (bdx, bdy) = (T::difference(bx, dx), T::difference(by, dy));
    let (cdx, cdy) = (T::difference(cx, dx), T::difference(cy, dy));
    let lift = |x: &T, y: &T| x.times(x).plus(&y.times(y));
    let cross = |x1: &T, y1: &T, x2: &T, y2: &T| x1.times(y2).minus(&x2.times(y1));
    let first = lift(&adx, &ady).times(&cross(&bdx, &bdy, &cdx, &cdy));
    let second = lift(&bdx, &bdy).times(&cross(&cdx, &cdy, &adx, &ady));
    let third = lift(&cdx, &cdy).times(&cross(&adx, &ady, &bdx, &bdy));
    first.plus(&second).plus(&third)
}

#[cfg(test)]
pub(super) mod tests;
