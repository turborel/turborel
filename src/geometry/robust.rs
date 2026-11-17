//! Exact orientation of finite binary64 coordinates. The common path
//! first proves the determinant's sign with a rounding-error bound, then
//! outward-rounded intervals. If neither proves a sign, bounded integer
//! arithmetic determines it. There is no geometric tolerance.

pub(super) mod distance;
mod integer;

use std::cmp::Ordering;

use super::shape::Point;
use integer::Integer;

/// Shewchuk, `predicates.c` (1996), `exactinit`'s `ccwerrboundA`:
/// (3 + 16u)u for binary64 unit roundoff u = 2^-53. The fast filter below
/// admits only normal products, determinant and bound, so the relative
/// error analysis encounters neither product underflow nor overflow.
const ORIENTATION_ERROR: f64 = 3.330_669_073_875_471_6e-16;

#[derive(Clone, Copy)]
struct Interval {
    low: f64,
    high: f64,
}

impl Interval {
    fn difference(left: f64, right: f64) -> Self {
        let value = left - right;
        Self { low: value.next_down(), high: value.next_up() }
    }

    fn multiply(self, other: Self) -> Self {
        let values = [self.low * other.low, self.low * other.high, self.high * other.low, self.high * other.high];
        if values.iter().any(|v| !v.is_finite()) {
            return Self { low: f64::NEG_INFINITY, high: f64::INFINITY };
        }
        let low = values.into_iter().fold(f64::INFINITY, f64::min).next_down();
        let high = values.into_iter().fold(f64::NEG_INFINITY, f64::max).next_up();
        Self { low, high }
    }

    fn add(self, other: Self) -> Self {
        Self { low: (self.low + other.low).next_down(), high: (self.high + other.high).next_up() }
    }

    fn subtract(self, other: Self) -> Self {
        Self { low: (self.low - other.high).next_down(), high: (self.high - other.low).next_up() }
    }
}

pub(super) fn orient([ax, ay]: Point, [bx, by]: Point, [cx, cy]: Point) -> Ordering {
    if let Some(sign) = filtered([ax, ay], [bx, by], [cx, cy]) {
        return sign;
    }
    let left = Interval::difference(ax, cx).multiply(Interval::difference(by, cy));
    let right = Interval::difference(ay, cy).multiply(Interval::difference(bx, cx));
    if (left.low - right.high).next_down() > 0.0_f64 {
        return Ordering::Greater;
    }
    if (left.high - right.low).next_up() < 0.0_f64 {
        return Ordering::Less;
    }
    exact([ax, ay], [bx, by], [cx, cy])
}

/// A subnormal coordinate difference is exact (differences are multiples
/// of binary64's least subnormal). Products must be normal; rounded-to-zero
/// products are never treated as exact. Cancellation and extreme values
/// fall through to the existing interval and integer proofs.
fn filtered([ax, ay]: Point, [bx, by]: Point, [cx, cy]: Point) -> Option<Ordering> {
    let left = (ax - cx) * (by - cy);
    let right = (ay - cy) * (bx - cx);
    let determinant = left - right;
    let error = ORIENTATION_ERROR * (left.abs() + right.abs());
    let normal = left.is_normal() && right.is_normal() && determinant.is_normal() && error.is_normal();
    if normal && determinant.abs() > error { Some(if determinant > 0.0_f64 { Ordering::Greater } else { Ordering::Less }) } else { None }
}

fn exact([ax, ay]: Point, [bx, by]: Point, [cx, cy]: Point) -> Ordering {
    let [ax, ay, bx, by, cx, cy] = [ax, ay, bx, by, cx, cy].map(Integer::coordinate);
    let left = ax.subtract(&cx).multiply(&by.subtract(&cy));
    let right = ay.subtract(&cy).multiply(&bx.subtract(&cx));
    left.subtract(&right).sign()
}

#[cfg(test)]
mod tests;
