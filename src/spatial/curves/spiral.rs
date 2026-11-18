//! Clothoids and spirals in their parameter planes, ISO/IEC 13249-3:2016
//! (DIS, SC32 N2593) §7.8.1 and §7.9.1. Both are parametrized by arc
//! length s. A clothoid of scale factor A is A√π (C(t), S(t)) at
//! t = s/(A√π), so its heading is s²/(2A²) and its curvature s/A². A
//! spiral starts at the origin heading along x, its curvature
//! κ(s) = κ₀ + (κ₁ - κ₀) f(s/L) for its type's transition f, an
//! implementation-defined choice (§7.9.1 D19); its heading has a closed
//! form and its position is integrated.

use super::fresnel::fresnel;
use super::numeric::{HALF, integrate};
use super::types::SpiralKind;
use std::f64::consts::{PI, TAU};

/// The heading, in radians, of a clothoid of scale `a` at distance s.
pub fn clothoid_heading(a: f64, s: f64) -> f64 {
    HALF * s * s / (a * a)
}

/// The parameter-plane point of a clothoid of scale `a` at distance s.
pub fn clothoid_point(a: f64, s: f64) -> (f64, f64) {
    let k = a.abs() * PI.sqrt();
    let (c, sine) = fresnel(s / k);
    (k * c, k * sine)
}

/// Two-thirds, from integrating the biquadratic transition's halves.
const TWO_THIRDS: f64 = 2.0 / 3.0;
/// Two, three: coefficients of the transitions.
const TWO: f64 = 2.0;
const THREE: f64 = 3.0;

/// The transition f(ξ) for length fraction ξ in [0, 1]: f(0) = 0,
/// f(1) = 1, each symmetric about the middle.
pub fn transition(kind: SpiralKind, xi: f64) -> f64 {
    match kind {
        SpiralKind::Clothoid => xi,
        SpiralKind::Bloss => xi * xi * TWO.mul_add(-xi, THREE),
        SpiralKind::Biquadratic => {
            if xi <= HALF {
                TWO * xi * xi
            } else {
                let rest = 1.0 - xi;
                (-TWO * rest).mul_add(rest, 1.0)
            }
        }
        SpiralKind::Sine => xi - (TAU * xi).sin() / TAU,
        SpiralKind::Cosine => HALF * (1.0 - (PI * xi).cos()),
    }
}

/// F(ξ) = ∫₀^ξ f, in closed form for each transition.
pub fn transition_integral(kind: SpiralKind, xi: f64) -> f64 {
    let square = xi * xi;
    match kind {
        SpiralKind::Clothoid => HALF * square,
        SpiralKind::Bloss => square * xi * (HALF.mul_add(-xi, 1.0)),
        SpiralKind::Biquadratic => {
            if xi <= HALF {
                TWO_THIRDS * square * xi
            } else {
                let rest = 1.0 - xi;
                TWO_THIRDS.mul_add(rest * rest * rest, xi - HALF)
            }
        }
        SpiralKind::Sine => HALF.mul_add(square, ((TAU * xi).cos() - 1.0) / (TAU * TAU)),
        SpiralKind::Cosine => HALF.mul_add(xi, -(PI * xi).sin() / TAU),
    }
}

/// A spiral's shape: length and end curvatures.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transition {
    /// The kind of transition.
    pub kind: SpiralKind,
    /// The length L.
    pub length: f64,
    /// The curvature κ₀ at the start.
    pub start: f64,
    /// The curvature κ₁ at the end.
    pub end: f64,
}

impl Transition {
    /// The curvature at distance s.
    pub fn curvature(&self, s: f64) -> f64 {
        let xi = self.fraction(s);
        (self.end - self.start).mul_add(transition(self.kind, xi), self.start)
    }

    /// The heading at distance s: ∫₀ˢ κ = κ₀ s + (κ₁ - κ₀) L F(s/L).
    pub fn heading(&self, s: f64) -> f64 {
        let xi = self.fraction(s);
        ((self.end - self.start) * self.length).mul_add(transition_integral(self.kind, xi), self.start * s)
    }

    /// The parameter-plane point at distance s: ∫₀ˢ (cos θ, sin θ).
    pub fn point(&self, s: f64) -> (f64, f64) {
        let x = integrate(|u| self.heading(u).cos(), 0.0, s, s);
        let y = integrate(|u| self.heading(u).sin(), 0.0, s, s);
        (x, y)
    }

    fn fraction(&self, s: f64) -> f64 {
        if self.length > 0.0 { (s / self.length).clamp(0.0, 1.0) } else { 0.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KINDS: [SpiralKind; 5] = [SpiralKind::Clothoid, SpiralKind::Bloss, SpiralKind::Biquadratic, SpiralKind::Sine, SpiralKind::Cosine];

    /// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.9.1 D19] Each
    /// transition rises from 0 to 1, and its closed-form integral matches
    /// quadrature of the transition at several fractions.
    #[test]
    fn transitions_and_their_integrals_agree() {
        for kind in KINDS {
            assert!(transition(kind, 0.0).abs() < 1e-15_f64, "{kind:?}");
            assert!((transition(kind, 1.0) - 1.0).abs() < 1e-15_f64, "{kind:?}");
            assert!((transition(kind, 0.5) - 0.5).abs() < 1e-15_f64, "{kind:?}");
            for xi in [0.1_f64, 0.3_f64, 0.5_f64, 0.7_f64, 1.0_f64] {
                let numeric = integrate(|u| transition(kind, u), 0.0, xi, 1.0);
                assert!((numeric - transition_integral(kind, xi)).abs() < 1e-14_f64, "{kind:?} at {xi}");
            }
        }
    }

    /// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.9.1] A spiral
    /// with equal curvatures is a circular arc: curvature 1/R over a
    /// half turn ends 2R across from its start.
    #[test]
    fn constant_curvature_spiral_is_a_circle() {
        let r = 3.0_f64;
        let spiral = Transition { kind: SpiralKind::Bloss, length: PI * r, start: 1.0 / r, end: 1.0 / r };
        let (x, y) = spiral.point(PI * r);
        assert!(x.abs() < 1e-12_f64 && (y - 2.0 * r).abs() < 1e-12_f64, "{x} {y}");
        assert!((spiral.curvature(1.0) - 1.0 / r).abs() < 1e-15_f64);
        let flat = Transition { kind: SpiralKind::Sine, length: 0.0, start: 0.0, end: 1.0 };
        assert_eq!(flat.heading(0.0), 0.0_f64);
    }

    /// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.2.11, §7.9.1]
    /// A clothoid-type spiral from zero curvature equals the clothoid of
    /// the Fresnel integrals with A² = R L.
    #[test]
    fn clothoid_spiral_matches_fresnel_clothoid() {
        let (length, radius) = (50.0_f64, 200.0_f64);
        let a = (length * radius).sqrt();
        let spiral = Transition { kind: SpiralKind::Clothoid, length, start: 0.0, end: 1.0 / radius };
        let (sx, sy) = spiral.point(length);
        let (cx, cy) = clothoid_point(a, length);
        assert!((sx - cx).abs() < 1e-11_f64 && (sy - cy).abs() < 1e-11_f64, "{sx} {sy} {cx} {cy}");
        assert!((spiral.heading(length) - clothoid_heading(a, length)).abs() < 1e-15_f64);
        assert!((clothoid_point(-a, length).0 - cx).abs() < 1e-15_f64);
    }
}
