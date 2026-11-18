//! \[DESIGN\] Numerical kernels the curve engines share: 3-vectors,
//! compensated sums, adaptive Gauss-Legendre quadrature, and the
//! cancellation-free circular segment term. Quadrature recursion is
//! bounded by [`MAX_DEPTH`] (RULES.md R-CF-6).

/// A point or vector in space.
pub type V3 = [f64; 3];

/// Componentwise a - b.
pub fn sub(a: V3, b: V3) -> V3 {
    let [ax, ay, az] = a;
    let [bx, by, bz] = b;
    [ax - bx, ay - by, az - bz]
}

/// Componentwise a + b.
pub fn add(a: V3, b: V3) -> V3 {
    let [ax, ay, az] = a;
    let [bx, by, bz] = b;
    [ax + bx, ay + by, az + bz]
}

/// a scaled by k.
pub fn scale(a: V3, k: f64) -> V3 {
    a.map(|v| v * k)
}

/// a + k b, fused per component.
pub fn axpy(a: V3, k: f64, b: V3) -> V3 {
    let [ax, ay, az] = a;
    let [bx, by, bz] = b;
    [k.mul_add(bx, ax), k.mul_add(by, ay), k.mul_add(bz, az)]
}

/// The dot product.
pub fn dot(a: V3, b: V3) -> f64 {
    let [ax, ay, az] = a;
    let [bx, by, bz] = b;
    ax.mul_add(bx, ay.mul_add(by, az * bz))
}

/// The cross product.
pub fn cross(a: V3, b: V3) -> V3 {
    let [ax, ay, az] = a;
    let [bx, by, bz] = b;
    [ay.mul_add(bz, -(az * by)), az.mul_add(bx, -(ax * bz)), ax.mul_add(by, -(ay * bx))]
}

/// The Euclidean length.
pub fn norm(a: V3) -> f64 {
    let [x, y, z] = a;
    x.hypot(y).hypot(z)
}

/// The length of a's projection on the xy plane.
pub fn norm2(a: V3) -> f64 {
    let [x, y, _] = a;
    x.hypot(y)
}

/// a with its z dropped to zero.
pub fn flat(a: V3) -> V3 {
    let [x, y, _] = a;
    [x, y, 0.0]
}

/// The planar cross product a.x b.y - a.y b.x.
pub fn cross2(a: V3, b: V3) -> f64 {
    let [ax, ay, _] = a;
    let [bx, by, _] = b;
    ax.mul_add(by, -(ay * bx))
}

/// a + t (b - a).
pub fn lerp(a: f64, b: f64, t: f64) -> f64 {
    t.mul_add(b - a, a)
}

/// A Neumaier-compensated sum, so long rings keep their small terms.
#[derive(Clone, Copy, Debug, Default)]
pub struct Sum {
    total: f64,
    carry: f64,
}

impl Sum {
    /// Adds `value`.
    pub fn add(&mut self, value: f64) {
        let next = self.total + value;
        self.carry += if self.total.abs() >= value.abs() { (self.total - next) + value } else { (value - next) + self.total };
        self.total = next;
    }

    /// The sum so far.
    pub fn value(self) -> f64 {
        self.total + self.carry
    }
}

/// Seven-point Gauss-Legendre abscissae on [-1, 1], nonnegative half
/// (Abramowitz and Stegun Table 25.4).
const NODES: [f64; 4] = [0.0, 0.405_845_151_377_397_2, 0.741_531_185_599_394_4, 0.949_107_912_342_758_5];
/// The weights matching [`NODES`].
const WEIGHTS: [f64; 4] = [0.417_959_183_673_469_4, 0.381_830_050_505_118_9, 0.279_705_391_489_276_7, 0.129_484_966_168_869_7];
/// Bisection depth limit for adaptive quadrature: 2^20 pieces at most.
pub const MAX_DEPTH: u32 = 20;
/// The relative error target of adaptive quadrature.
pub const QUADRATURE_TOLERANCE: f64 = 1e-13;
/// One half.
pub const HALF: f64 = 0.5;

/// The seven-point rule over [a, b].
fn rule(f: &mut impl FnMut(f64) -> f64, a: f64, b: f64) -> f64 {
    let middle = HALF * (a + b);
    let radius = HALF * (b - a);
    let mut total = Sum::default();
    for (node, weight) in NODES.iter().zip(WEIGHTS) {
        if *node > 0.0_f64 {
            total.add(weight * (f(radius.mul_add(-node, middle)) + f(radius.mul_add(*node, middle))));
        } else {
            total.add(weight * f(middle));
        }
    }
    total.value() * radius
}

/// The integral of `f` over [a, b], bisected until halves agree with the
/// whole to [`QUADRATURE_TOLERANCE`] of `scale` or [`MAX_DEPTH`] is met.
pub fn integrate(mut f: impl FnMut(f64) -> f64, a: f64, b: f64, magnitude: f64) -> f64 {
    let whole = rule(&mut f, a, b);
    let goal = QUADRATURE_TOLERANCE * magnitude.abs().max(whole.abs()).max(f64::MIN_POSITIVE);
    refine(&mut f, Interval { a, b, whole, goal }, 0)
}

/// A subinterval, its seven-point estimate and its error budget.
#[derive(Clone, Copy)]
struct Interval {
    a: f64,
    b: f64,
    whole: f64,
    goal: f64,
}

fn refine(f: &mut impl FnMut(f64) -> f64, at: Interval, depth: u32) -> f64 {
    let Interval { a, b, whole, goal } = at;
    let middle = HALF * (a + b);
    let left = rule(f, a, middle);
    let right = rule(f, middle, b);
    let halves = left + right;
    if depth >= MAX_DEPTH || (halves - whole).abs() <= goal {
        return halves;
    }
    let next = depth.saturating_add(1);
    let half = goal * HALF;
    refine(f, Interval { a, b: middle, whole: left, goal: half }, next)
        + refine(f, Interval { a: middle, b, whole: right, goal: half }, next)
}

/// Below this |θ| the series of θ - sin θ replaces the subtraction.
const SERIES_BELOW: f64 = 1e-2;
/// 3!, 5!, 7!, 9! and 11! for the series of θ - sin θ.
const FACTORIALS: [f64; 5] = [6.0, 120.0, 5040.0, 362_880.0, 39_916_800.0];

/// θ - sin θ without cancellation: twice the signed area of a unit
/// circle's segment of angle θ.
pub fn theta_minus_sin(theta: f64) -> f64 {
    if theta.abs() >= SERIES_BELOW {
        return theta - theta.sin();
    }
    let square = theta * theta;
    let mut power = theta * square;
    let mut total = Sum::default();
    let mut sign = 1.0_f64;
    for factorial in FACTORIALS {
        total.add(sign * power / factorial);
        power *= square;
        sign = -sign;
    }
    total.value()
}

/// The root of increasing `f` in [lo, hi] for target `goal`, by bisection
/// to the resolution of f64 (at most [`BISECTIONS`] halvings).
pub fn invert(mut f: impl FnMut(f64) -> f64, goal: f64, lo: f64, hi: f64) -> f64 {
    let (mut low, mut high) = (lo, hi);
    for _ in 0..BISECTIONS {
        let middle = HALF * (low + high);
        if middle <= low || middle >= high {
            break;
        }
        if f(middle) < goal {
            low = middle;
        } else {
            high = middle;
        }
    }
    HALF * (low + high)
}

/// Enough halvings to exhaust a double's 52-bit mantissa from any span.
pub const BISECTIONS: u32 = 200;

/// Relative tolerance for treating two computed points as equal (§4.2.1
/// allows an implementation-defined one): 10⁻¹² of the coordinates'
/// magnitude, far above the rounding of a placement's arithmetic.
pub const POINT_TOLERANCE: f64 = 1e-12;

/// Whether a and b are equal within [`POINT_TOLERANCE`] of their size.
pub fn same(a: V3, b: V3) -> bool {
    let size = norm(a).max(norm(b)).max(1.0);
    norm(sub(a, b)) <= POINT_TOLERANCE * size
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] Adaptive quadrature integrates smooth and
    /// oscillating functions to the target precision.
    #[test]
    fn quadrature_converges() {
        let sine = integrate(f64::sin, 0.0, std::f64::consts::PI, 1.0);
        assert!((sine - 2.0).abs() < 1e-12_f64, "{sine}");
        let wavy = integrate(|x| (40.0 * x).cos(), 0.0, 3.0, 1.0);
        assert!((wavy - (120.0_f64).sin() / 40.0).abs() < 1e-12_f64, "{wavy}");
        let cubic = integrate(|x| x * x * x, -1.0, 2.0, 1.0);
        assert!((cubic - 3.75).abs() < 1e-13_f64);
    }

    /// Verifies: [DESIGN] θ - sin θ keeps full precision near zero, where
    /// the series and the subtraction meet within rounding.
    #[test]
    fn segment_term_is_accurate_near_zero() {
        let tiny = theta_minus_sin(1e-6);
        assert!((tiny / (1e-18 / 6.0) - 1.0).abs() < 1e-9_f64);
        let edge = 0.009_999_999_f64;
        let series = theta_minus_sin(edge);
        let direct = edge - edge.sin();
        assert!((series - direct).abs() < 1e-16_f64);
        assert!((theta_minus_sin(2.0) - (2.0 - 2.0_f64.sin())).abs() < 1e-16_f64);
    }

    /// Verifies: [DESIGN] Compensated sums keep small terms beside large
    /// ones; bisection inverts an increasing function; vector helpers
    /// agree with their definitions.
    #[test]
    fn helpers_hold_their_definitions() {
        let mut sum = Sum::default();
        for v in [1e16_f64, 1.0_f64, -1e16_f64, 1.0_f64] {
            sum.add(v);
        }
        assert_eq!(sum.value(), 2.0_f64);
        let root = invert(|x| x * x, 2.0, 0.0, 2.0);
        assert!((root - 2.0_f64.sqrt()).abs() < 1e-15_f64);
        assert_eq!(cross([1.0_f64, 0.0_f64, 0.0_f64], [0.0_f64, 1.0_f64, 0.0_f64]), [0.0_f64, 0.0_f64, 1.0_f64]);
        assert_eq!(dot([1.0_f64, 2.0_f64, 3.0_f64], [4.0_f64, 5.0_f64, 6.0_f64]), 32.0_f64);
        assert_eq!(norm2([3.0_f64, 4.0_f64, 12.0_f64]), 5.0_f64);
        assert!(same([1e6_f64, 1.0_f64, 0.0_f64], [1e6_f64 + 1e-7_f64, 1.0_f64, 0.0_f64]));
        assert!(!same([1.0_f64, 1.0_f64, 0.0_f64], [1.0_f64, 1.001_f64, 0.0_f64]));
        assert_eq!(lerp(2.0, 4.0, 0.25), 2.5_f64);
        assert_eq!(axpy([1.0_f64, 1.0_f64, 1.0_f64], 2.0, [1.0_f64, 2.0_f64, 3.0_f64]), [3.0_f64, 5.0_f64, 7.0_f64]);
        assert_eq!(cross2([1.0_f64, 0.0_f64, 9.0_f64], [0.0_f64, 2.0_f64, 9.0_f64]), 2.0_f64);
        assert_eq!(flat([1.0_f64, 2.0_f64, 3.0_f64]), [1.0_f64, 2.0_f64, 0.0_f64]);
        assert_eq!(add(scale([1.0_f64, 2.0_f64, 3.0_f64], 2.0), sub([0.0_f64; 3], [1.0_f64; 3])), [1.0_f64, 3.0_f64, 5.0_f64]);
    }
}
