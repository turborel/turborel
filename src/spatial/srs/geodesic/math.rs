//! Angle arithmetic in degrees that stays exact where it can, after
//! Karney, "Algorithms for geodesics" (J. Geodesy 87, 2013) and its
//! reference implementation: exact reduction of angles, sines and
//! cosines of multiples of 90° exact, an error-free sum, and the
//! double-double accumulator the polygon area adds into.

use std::f64::consts::PI;

use super::super::{HALF, TWO};

/// A right angle, a half turn and a full turn, degrees.
pub(super) const QUARTER: f64 = 90.0;
pub(super) const HALF_TURN: f64 = 180.0;
pub(super) const TURN: f64 = 360.0;
/// Angles are rounded to multiples of 2⁻⁵⁷ of 1/16 degree near zero, so
/// a tiny latitude does not give a needlessly inaccurate geodesic.
const ROUNDING: f64 = 1.0 / 16.0;

/// s = u + v and the rounding error t of that sum (Knuth's TwoSum).
pub(super) fn sum(u: f64, v: f64) -> (f64, f64) {
    let s = u + v;
    let up = s - v;
    let vpp = s - up;
    let error = -((up - u) + (vpp - v));
    (s, if s == 0.0_f64 { s } else { error })
}

/// The polynomial with coefficients `p`, highest degree first, at `x`.
pub(super) fn polyval(p: &[f64], x: f64) -> f64 {
    p.iter().fold(0.0_f64, |y, c| y * x + c)
}

/// A latitude, NaN beyond ±90°.
pub(super) fn latitude(x: f64) -> f64 {
    if x.abs() > QUARTER { f64::NAN } else { x }
}

/// `x` reduced to [−180°, 180°], exactly.
fn remainder(x: f64) -> f64 {
    let y = x % TURN;
    if y < -HALF_TURN {
        y + TURN
    } else if y > HALF_TURN {
        y - TURN
    } else {
        y
    }
}

/// `x` reduced to (−180°, 180°], keeping the sign of x at ±180°.
pub(super) fn normalize(x: f64) -> f64 {
    let y = remainder(x);
    if y.abs() == HALF_TURN { HALF_TURN.copysign(x) } else { y }
}

/// y − x reduced to [−180°, 180°], and the rounding error of that.
pub(super) fn difference(x: f64, y: f64) -> (f64, f64) {
    let (d, t) = sum(remainder(-x), remainder(y));
    let (d, t2) = sum(remainder(d), t);
    let d = if d == 0.0_f64 || d.abs() == HALF_TURN { d.copysign(if t2 == 0.0_f64 { y - x } else { -t2 }) } else { d };
    (d, t2)
}

/// `x` rounded so that values near zero are coarse, which makes very
/// small latitudes and azimuths exact zero when they should be.
pub(super) fn round(x: f64) -> f64 {
    let y = x.abs();
    let w = ROUNDING - y;
    let y = if w > 0.0_f64 { ROUNDING - w } else { y };
    y.copysign(x)
}

/// Sine and cosine of `x` degrees, exact at multiples of 90°.
pub(super) fn sincosd(x: f64) -> (f64, f64) {
    sincosd_error(x, 0.0_f64)
}

/// Sine and cosine of `x` + `t` degrees, `t` a small correction.
pub(super) fn sincosd_error(x: f64, t: f64) -> (f64, f64) {
    let rem = x % TURN;
    let turns = (rem / QUARTER).round();
    // |rem − 90q| ≤ 45 and rem, 90q are within a factor two: exact (Sterbenz).
    let reduced = if t == 0.0_f64 { rem - QUARTER * turns } else { round(rem - QUARTER * turns + t) };
    let (sine, cosine) = reduced.to_radians().sin_cos();
    let quadrant = turns.rem_euclid(4.0_f64);
    let (sin, cos) = if quadrant < HALF {
        (sine, cosine)
    } else if quadrant < 1.0_f64 + HALF {
        (cosine, -sine)
    } else if quadrant < TWO + HALF {
        (-sine, -cosine)
    } else {
        (-cosine, sine)
    };
    let sin = if sin == 0.0_f64 { sin.copysign(x) } else { sin };
    (sin, cos + 0.0_f64)
}

/// atan2(y, x) in degrees, exact for the quadrant boundaries.
pub(super) fn atan2d(y: f64, x: f64) -> f64 {
    let (y, x, swapped) = if y.abs() > x.abs() { (x, y, true) } else { (y, x, false) };
    let (x, negative) = if x.is_sign_negative() { (-x, true) } else { (x, false) };
    let angle = y.atan2(x).to_degrees();
    match (swapped, negative) {
        (false, false) => angle,
        (false, true) => HALF_TURN.copysign(y) - angle,
        (true, false) => QUARTER - angle,
        (true, true) => -QUARTER + angle,
    }
}

/// (s, c) scaled to unit length.
pub(super) fn norm(s: f64, c: f64) -> (f64, f64) {
    let r = s.hypot(c);
    (s / r, c / r)
}

/// Radians per degree.
pub(super) const DEGREE: f64 = PI / HALF_TURN;

/// A sum carried as a double-double, s + t.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Accumulator {
    s: f64,
    t: f64,
}

impl Accumulator {
    /// Adds `y`.
    pub(super) fn add(&mut self, y: f64) {
        let (partial, low) = sum(y, self.t);
        let (high, carry) = sum(partial, self.s);
        self.s = high;
        self.t = carry;
        if self.s == 0.0_f64 {
            self.s = low;
        } else {
            self.t += low;
        }
    }

    /// The sum.
    pub(super) fn value(self) -> f64 {
        self.s
    }

    /// Reduces the sum to [−y/2, y/2].
    pub(super) fn remainder(&mut self, y: f64) {
        self.s -= y * (self.s / y).round_ties_even();
        self.add(0.0_f64);
    }

    /// Negates the sum.
    pub(super) fn negate(&mut self) {
        self.s = -self.s;
        self.t = -self.t;
    }
}
