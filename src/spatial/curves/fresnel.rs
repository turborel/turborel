//! \[DESIGN\] The normalized Fresnel integrals C(x) = ∫₀ˣ cos(πt²/2) dt
//! and S(x) = ∫₀ˣ sin(πt²/2) dt that place a clothoid, ISO/IEC 13249-3:2016
//! (DIS, SC32 N2593) §4.2.11. The power series serves near zero, adaptive
//! quadrature the middle, and the auxiliary functions f and g (DLMF 7.12)
//! beyond, so no range loses more than a few units in the last place.

use super::numeric::{HALF, Sum, integrate};
use std::f64::consts::{FRAC_PI_2, PI};

/// The power series is used up to here; its largest term is e^(πx²/2).
const SERIES_UP_TO: f64 = 2.0;
/// The asymptotic expansion is used from here; πx² ≥ 113 makes its
/// terms fall below 10⁻¹⁷ before they diverge.
const ASYMPTOTIC_FROM: f64 = 6.0;
/// Series terms beyond this are below a double's resolution for |x| ≤ 2.
const SERIES_TERMS: u32 = 40;
/// Asymptotic terms are summed while they shrink, at most this many.
const ASYMPTOTIC_TERMS: u32 = 30;
/// The period of cos(πt²/2) in t², used to reduce its argument exactly.
const PERIOD: f64 = 4.0;
/// Three and four: factors of the series' coefficients.
const THREE: f64 = 3.0;
const FOUR: f64 = 4.0;

/// C(x) and S(x).
pub fn fresnel(x: f64) -> (f64, f64) {
    let sign = if x < 0.0_f64 { -1.0_f64 } else { 1.0_f64 };
    let a = x.abs();
    let (c, s) = if a <= SERIES_UP_TO {
        series(a)
    } else if a < ASYMPTOTIC_FROM {
        let (c0, s0) = series(SERIES_UP_TO);
        let c = integrate(|t| phase(t).cos(), SERIES_UP_TO, a, 1.0);
        let s = integrate(|t| phase(t).sin(), SERIES_UP_TO, a, 1.0);
        (c0 + c, s0 + s)
    } else {
        asymptotic(a)
    };
    (sign * c, sign * s)
}

/// πx²/2, its x² reduced modulo 4 with the rounding of x² kept, so the
/// phase stays accurate for large x.
fn phase(x: f64) -> f64 {
    let high = x * x;
    let low = x.mul_add(x, -high);
    let reduced = high - PERIOD * (high / PERIOD).floor();
    FRAC_PI_2 * (reduced + low)
}

/// The Maclaurin series: C = Σ (-1)ⁿ (π/2)²ⁿ x⁴ⁿ⁺¹ / ((2n)! (4n+1)) and
/// S = Σ (-1)ⁿ (π/2)²ⁿ⁺¹ x⁴ⁿ⁺³ / ((2n+1)! (4n+3)).
fn series(x: f64) -> (f64, f64) {
    let power = FRAC_PI_2 * x * x;
    let (mut cosine, mut sine) = (Sum::default(), Sum::default());
    // term = powerᵏ / k!, k counting both series' terms in turn.
    let mut term = 1.0_f64;
    let mut k = 0.0_f64;
    for n in 0..SERIES_TERMS {
        let even = term / FOUR.mul_add(f64::from(n), 1.0);
        let sign = if n % 2 == 0 { 1.0_f64 } else { -1.0_f64 };
        cosine.add(sign * even);
        k += 1.0_f64;
        term *= power / k;
        sine.add(sign * term / FOUR.mul_add(f64::from(n), THREE));
        k += 1.0_f64;
        term *= power / k;
    }
    (x * cosine.value(), x * sine.value())
}

/// C = ½ + f sin φ - g cos φ and S = ½ - f cos φ - g sin φ, φ = πx²/2,
/// with f ~ (1/(πx)) Σ (-1)ᵐ (4m-1)!! / (πx²)²ᵐ and
/// g ~ (1/(π²x³)) Σ (-1)ᵐ (4m+1)!! / (πx²)²ᵐ (DLMF 7.12.2, 7.12.3).
fn asymptotic(x: f64) -> (f64, f64) {
    let w = PI * x * x;
    let w2 = w * w;
    let (mut f, mut g) = (Sum::default(), Sum::default());
    let (mut tf, mut tg) = (1.0_f64, 1.0_f64);
    for m in 1..=ASYMPTOTIC_TERMS {
        f.add(tf);
        g.add(tg);
        let m4 = FOUR * f64::from(m);
        let next_f = -tf * (m4 - THREE) * (m4 - 1.0_f64) / w2;
        let next_g = -tg * (m4 - 1.0_f64) * (m4 + 1.0_f64) / w2;
        if next_f.abs() >= tf.abs() || next_f.abs() < f64::EPSILON * f64::EPSILON {
            break;
        }
        tf = next_f;
        tg = next_g;
    }
    let f = f.value() / (PI * x);
    let g = g.value() / (PI * PI * x * x * x);
    let (sin, cos) = phase(x).sin_cos();
    (HALF + f.mul_add(sin, -(g * cos)), HALF - f.mul_add(cos, g * sin))
}

/// The limits of C and S at infinity.
pub const LIMIT: f64 = HALF;

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.2.11] Fresnel
    /// integrals match tabulated values (Abramowitz and Stegun Table 7.7)
    /// in each of the three ranges, and are odd.
    #[test]
    fn fresnel_integrals_match_tables() {
        let cases = [
            (0.5_f64, 0.492_344_225_871_446_4_f64, 0.064_732_432_859_999_27_f64),
            (1.0_f64, 0.779_893_400_376_822_9_f64, 0.438_259_147_390_354_76_f64),
            (2.0_f64, 0.488_253_406_075_340_73_f64, 0.343_415_678_363_698_24_f64),
            (3.0_f64, 0.605_720_789_297_685_6_f64, 0.496_312_998_967_375_f64),
            (5.0_f64, 0.563_631_188_704_012_2_f64, 0.499_191_381_917_116_87_f64),
            (10.0_f64, 0.499_898_694_205_515_7_f64, 0.468_169_978_584_882_24_f64),
        ];
        for (x, c, s) in cases {
            let (fc, fs) = fresnel(x);
            assert!((fc - c).abs() < 1e-13_f64, "C({x}) = {fc}, want {c}");
            assert!((fs - s).abs() < 1e-13_f64, "S({x}) = {fs}, want {s}");
            let (nc, ns) = fresnel(-x);
            assert_eq!((nc, ns), (-fc, -fs));
        }
        let (c, s) = fresnel(0.0);
        assert_eq!((c, s), (0.0_f64, 0.0_f64));
        let (far_c, far_s) = fresnel(1e4);
        assert!((far_c - LIMIT).abs() < 1e-4_f64 && (far_s - LIMIT).abs() < 1e-4_f64);
        let (c6, _) = fresnel(6.0);
        let (c6m, _) = fresnel(5.999_999_999);
        assert!((c6 - c6m).abs() < 1e-8_f64, "ranges meet continuously");
    }

    /// Verifies: [DESIGN] The reduced phase equals πx²/2 modulo 2π.
    #[test]
    fn phase_reduction_is_exact_enough() {
        let x = 1234.5_f64;
        let direct = FRAC_PI_2 * x * x;
        assert!((phase(x).sin() - direct.sin()).abs() < 1e-9_f64);
        assert!((phase(1.0) - FRAC_PI_2).abs() < 1e-15_f64);
    }
}
