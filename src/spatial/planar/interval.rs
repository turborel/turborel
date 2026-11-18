//! \[DESIGN\] Outward-rounded interval arithmetic, the middle stage of
//! every predicate, and the arithmetic both later stages share. Each
//! operation widens its rounded result by one unit in the last place
//! each way, so the exact value of the expression always lies inside.

use std::cmp::Ordering;

use super::dyadic::Dyadic;

/// The operations a determinant needs, implemented by intervals and by
/// exact dyadic values, so each predicate is written once for both.
pub(crate) trait Arithmetic: Sized {
    /// The value `left - right`.
    fn difference(left: f64, right: f64) -> Self;
    /// The sum.
    fn plus(&self, other: &Self) -> Self;
    /// The difference.
    fn minus(&self, other: &Self) -> Self;
    /// The product.
    fn times(&self, other: &Self) -> Self;
}

impl Arithmetic for Dyadic {
    fn difference(left: f64, right: f64) -> Self {
        Dyadic::of(left).minus(&Dyadic::of(right))
    }

    fn plus(&self, other: &Self) -> Self {
        Dyadic::plus(self, other)
    }

    fn minus(&self, other: &Self) -> Self {
        Dyadic::minus(self, other)
    }

    fn times(&self, other: &Self) -> Self {
        Dyadic::times(self, other)
    }
}

/// A closed interval containing an exact value.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Interval {
    low: f64,
    high: f64,
}

/// Every real number, the result once any bound is not a number.
const UNBOUNDED: Interval = Interval { low: f64::NEG_INFINITY, high: f64::INFINITY };

impl Interval {
    fn bounded(low: f64, high: f64) -> Self {
        if low.is_nan() || high.is_nan() { UNBOUNDED } else { Self { low: low.next_down(), high: high.next_up() } }
    }

    /// The sign the interval proves, if it excludes zero.
    pub(crate) fn sign(self) -> Option<Ordering> {
        if self.low > 0.0_f64 {
            Some(Ordering::Greater)
        } else if self.high < 0.0_f64 {
            Some(Ordering::Less)
        } else {
            None
        }
    }
}

impl Arithmetic for Interval {
    fn difference(left: f64, right: f64) -> Self {
        let value = left - right;
        Self::bounded(value, value)
    }

    fn plus(&self, other: &Self) -> Self {
        Self::bounded(self.low + other.low, self.high + other.high)
    }

    fn minus(&self, other: &Self) -> Self {
        Self::bounded(self.low - other.high, self.high - other.low)
    }

    fn times(&self, other: &Self) -> Self {
        let values = [self.low * other.low, self.low * other.high, self.high * other.low, self.high * other.high];
        if values.iter().any(|value| value.is_nan()) {
            return UNBOUNDED;
        }
        let low = values.into_iter().fold(f64::INFINITY, f64::min);
        let high = values.into_iter().fold(f64::NEG_INFINITY, f64::max);
        Self::bounded(low, high)
    }
}
