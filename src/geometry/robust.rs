//! Exact orientation of finite binary64 coordinates. The common path
//! proves the determinant's sign with outward-rounded intervals. If the
//! interval includes zero, overflows or underflows, bounded integer
//! arithmetic determines the sign. There is no geometric tolerance.

pub(super) mod distance;
mod integer;

use std::cmp::Ordering;

use super::shape::Point;
use integer::Integer;

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

fn exact([ax, ay]: Point, [bx, by]: Point, [cx, cy]: Point) -> Ordering {
    let [ax, ay, bx, by, cx, cy] = [ax, ay, bx, by, cx, cy].map(Integer::coordinate);
    let left = ax.subtract(&cx).multiply(&by.subtract(&cy));
    let right = ay.subtract(&cy).multiply(&bx.subtract(&cx));
    left.subtract(&right).sign()
}

#[cfg(test)]
mod tests;
