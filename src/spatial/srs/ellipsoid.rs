//! The ellipsoid of revolution of a <spheroid> (ISO/IEC 13249-3:2016 (DIS,
//! SC32 N2593) §14.1.9 Description 1k), its auxiliary latitudes, the
//! conversion between geodetic and geocentric coordinates (EPSG Guidance
//! Note 7-2, method 9602), and the seven-parameter Helmert datum shift
//! TOWGS84 carries (method 9606, position vector).
//!
//! Conformal latitude follows Karney, "Transverse Mercator with an
//! accuracy of a few nanometers" (J. Geodesy 85, 2011), through
//! τ' = tan χ, accurate near the poles where series in φ lose digits.

use std::f64::consts::FRAC_PI_2;

use super::fault::{Fault, Outcome};
use super::model::Helmert;
use super::{HALF, TWO};

/// Newton steps of the inverse latitude functions; each converges in two
/// or three for terrestrial eccentricities (Karney 2011 §2.4).
const NEWTON_STEPS: usize = 8;
/// Stop iterating when a step changes τ by less than this many times √ε,
/// relative (Karney 2011 §2.4).
const TAU_TOLERANCE: f64 = 0.1;
/// Beyond this |τ'| the first guess uses the polar asymptote.
const POLAR_TAU: f64 = 70.0;
/// Steps of the geocentric to geodetic iteration (Bowring 1985), each
/// gaining about three orders of magnitude.
const BOWRING_STEPS: usize = 6;
/// Radians per arc-second, of the TOWGS84 rotations.
const ARC_SECOND: f64 = std::f64::consts::PI / 648_000.0;
/// Parts per million, of the TOWGS84 scale difference.
const PPM: f64 = 1e-6;
/// The semi-major axis of WGS 84, metres (NIMA TR8350.2).
pub const WGS84_SEMI_MAJOR: f64 = 6_378_137.0;
/// The inverse flattening of WGS 84 (NIMA TR8350.2).
pub const WGS84_INVERSE_FLATTENING: f64 = 298.257_223_563;

/// An ellipsoid of revolution and the quantities derived from it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ellipsoid {
    /// Semi-major axis a, metres.
    pub a: f64,
    /// Flattening f; zero for a sphere.
    pub f: f64,
    /// Semi-minor axis b = a(1 − f).
    pub b: f64,
    /// First eccentricity squared, e² = f(2 − f).
    pub e2: f64,
    /// First eccentricity e.
    pub e: f64,
    /// Second eccentricity squared, e'² = e²/(1 − e²).
    pub ep2: f64,
    /// Third flattening n = f/(2 − f).
    pub n: f64,
}

impl Ellipsoid {
    /// The ellipsoid with semi-major axis `a` and inverse flattening `rf`,
    /// a sphere when `rf` is zero.
    ///
    /// # Errors
    ///
    /// F02 invalid argument if `a` is not above zero or `rf` is neither
    /// zero nor above one.
    pub fn new(a: f64, rf: f64) -> Outcome<Ellipsoid> {
        let sphere = rf.abs() <= 0.0_f64;
        if !(a > 0.0_f64 && a.is_finite() && rf.is_finite() && (sphere || rf > 1.0_f64)) {
            return Err(Fault::argument("[ISO/IEC 13249-3:2016 §14.1.9 Description 1k] an ellipsoid needs a > 0 and 1/f of 0 or above 1"));
        }
        let f = if sphere { 0.0_f64 } else { 1.0_f64 / rf };
        let e2 = f * (TWO - f);
        Ok(Ellipsoid { a, f, b: a * (1.0_f64 - f), e2, e: e2.sqrt(), ep2: e2 / (1.0_f64 - e2), n: f / (TWO - f) })
    }

    /// WGS 84.
    pub fn wgs84() -> Ellipsoid {
        let f = 1.0_f64 / WGS84_INVERSE_FLATTENING;
        let e2 = f * (TWO - f);
        let a = WGS84_SEMI_MAJOR;
        Ellipsoid { a, f, b: a * (1.0_f64 - f), e2, e: e2.sqrt(), ep2: e2 / (1.0_f64 - e2), n: f / (TWO - f) }
    }

    /// e·atanh(e x), the term isometric and conformal latitude subtract.
    fn eatanhe(&self, x: f64) -> f64 {
        self.e * (self.e * x).atanh()
    }

    /// τ' = tan χ of the conformal latitude χ, from τ = tan φ.
    pub fn conformal_tan(&self, tau: f64) -> f64 {
        if !tau.is_finite() {
            return tau;
        }
        let tau1 = tau.hypot(1.0_f64);
        let sig = self.eatanhe(tau / tau1).sinh();
        sig.hypot(1.0_f64) * tau - sig * tau1
    }

    /// τ = tan φ from τ' = tan χ, by Newton's method (Karney 2011 eq. 19).
    pub fn geodetic_tan(&self, taup: f64) -> f64 {
        let e2m = 1.0_f64 - self.e2;
        let mut tau = if taup.abs() > POLAR_TAU { taup * self.eatanhe(1.0_f64).exp() } else { taup / e2m };
        if !tau.is_finite() {
            return tau;
        }
        let tolerance = TAU_TOLERANCE * f64::EPSILON.sqrt() * taup.abs().max(1.0_f64);
        for _ in 0..NEWTON_STEPS {
            let taupa = self.conformal_tan(tau);
            let step = (taup - taupa) * (1.0_f64 + e2m * tau * tau) / (e2m * tau.hypot(1.0_f64) * taupa.hypot(1.0_f64));
            tau += step;
            if step.abs() < tolerance {
                break;
            }
        }
        tau
    }

    /// Isometric latitude ψ = asinh(tan χ) of the geodetic latitude `phi`.
    pub fn isometric(&self, phi: f64) -> f64 {
        self.conformal_tan(phi.tan()).asinh()
    }

    /// The geodetic latitude whose isometric latitude is `psi`.
    pub fn from_isometric(&self, psi: f64) -> f64 {
        self.geodetic_tan(psi.sinh()).atan()
    }

    /// Conformal latitude χ of `phi`.
    pub fn conformal(&self, phi: f64) -> f64 {
        self.conformal_tan(phi.tan()).atan()
    }

    /// The geodetic latitude whose conformal latitude is `chi`.
    pub fn from_conformal(&self, chi: f64) -> f64 {
        self.geodetic_tan(chi.tan()).atan()
    }

    /// atanh(e x)/e, which tends to x on a sphere.
    fn atanh_over_e(&self, x: f64) -> f64 {
        if self.e > 0.0_f64 { (self.e * x).atanh() / self.e } else { x }
    }

    /// q of `phi` (EPSG Guidance Note 7-2, Albers and Lambert azimuthal
    /// equal area): q = (1 − e²)(sin φ/(1 − e² sin² φ) + atanh(e sin φ)/e).
    pub fn q(&self, phi: f64) -> f64 {
        let s = phi.sin();
        (1.0_f64 - self.e2) * (s / (1.0_f64 - self.e2 * s * s) + self.atanh_over_e(s))
    }

    /// The latitude whose q is `q`, by Newton's method (Snyder 1987,
    /// eq. 3-16), from the sphere's arcsine as the first guess.
    pub fn from_q(&self, q: f64) -> f64 {
        let pole = self.q(FRAC_PI_2);
        let mut phi = (q / pole).clamp(-1.0_f64, 1.0_f64).asin();
        for _ in 0..NEWTON_STEPS {
            if phi.abs() > FRAC_PI_2 - f64::EPSILON {
                break;
            }
            let (s, c) = phi.sin_cos();
            let w = 1.0_f64 - self.e2 * s * s;
            let step = w * w / (TWO * c) * (q / (1.0_f64 - self.e2) - s / w - self.atanh_over_e(s));
            phi += step;
            if step.abs() < f64::EPSILON {
                break;
            }
        }
        phi
    }

    /// ν, the radius of curvature in the prime vertical at `phi`.
    pub fn nu(&self, phi: f64) -> f64 {
        let s = phi.sin();
        self.a / (1.0_f64 - self.e2 * s * s).sqrt()
    }

    /// m = cos φ/√(1 − e² sin² φ) of `phi`, the parallel's radius over a.
    pub fn m(&self, phi: f64) -> f64 {
        let (s, c) = phi.sin_cos();
        c / (1.0_f64 - self.e2 * s * s).sqrt()
    }

    /// Geocentric X, Y, Z of longitude `lambda`, latitude `phi` (radians)
    /// and ellipsoidal height `h` (EPSG method 9602).
    pub fn geocentric(&self, lambda: f64, phi: f64, h: f64) -> [f64; 3] {
        let nu = self.nu(phi);
        let (sp, cp) = phi.sin_cos();
        let (sl, cl) = lambda.sin_cos();
        [(nu + h) * cp * cl, (nu + h) * cp * sl, ((1.0_f64 - self.e2) * nu + h) * sp]
    }

    /// Longitude, latitude (radians) and ellipsoidal height of geocentric
    /// `xyz`, by Bowring's iteration on the parametric latitude.
    pub fn geodetic(&self, xyz: [f64; 3]) -> [f64; 3] {
        let [x, y, z] = xyz;
        let radius = x.hypot(y);
        let lambda = y.atan2(x);
        if radius <= 0.0_f64 {
            return [lambda, FRAC_PI_2.copysign(z), z.abs() - self.b];
        }
        let mut beta = z.atan2((1.0_f64 - self.f) * radius);
        let mut phi = beta;
        for _ in 0..BOWRING_STEPS {
            let (sb, cb) = beta.sin_cos();
            phi = (z + self.ep2 * self.b * sb * sb * sb).atan2(radius - self.e2 * self.a * cb * cb * cb);
            let next = ((1.0_f64 - self.f) * phi.sin()).atan2(phi.cos());
            let settled = (next - beta).abs() < f64::EPSILON;
            beta = next;
            if settled {
                break;
            }
        }
        let (sp, cp) = phi.sin_cos();
        let height = radius * cp + z * sp - self.a * (1.0_f64 - self.e2 * sp * sp).sqrt();
        [lambda, phi, height]
    }
}

/// A Helmert transformation prepared for use: its translation, the
/// small-angle rotation matrix scaled by 1 + ds, and that matrix's inverse.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shift {
    translation: [f64; 3],
    matrix: [[f64; 3]; 3],
    inverse: [[f64; 3]; 3],
}

impl Shift {
    /// The shift TOWGS84 `helmert` gives, in the position vector
    /// convention of EPSG method 9606:
    /// X' = T + (1 + ds) R X with R = [[1, −rz, ry], [rz, 1, −rx], [−ry, rx, 1]].
    pub fn new(helmert: &Helmert) -> Shift {
        let [rx, ry, rz] = helmert.rotation.map(|r| r * ARC_SECOND);
        let s = 1.0_f64 + helmert.scale * PPM;
        let matrix = [[s, -rz * s, ry * s], [rz * s, s, -rx * s], [-ry * s, rx * s, s]];
        Shift { translation: helmert.translation, matrix, inverse: invert(matrix) }
    }

    /// `xyz` shifted to WGS 84.
    pub fn forward(&self, xyz: [f64; 3]) -> [f64; 3] {
        let [x, y, z] = apply(&self.matrix, xyz);
        let [tx, ty, tz] = self.translation;
        [x + tx, y + ty, z + tz]
    }

    /// `xyz` on WGS 84 shifted back, the exact inverse of [`Shift::forward`].
    pub fn inverse(&self, xyz: [f64; 3]) -> [f64; 3] {
        let [x, y, z] = xyz;
        let [tx, ty, tz] = self.translation;
        apply(&self.inverse, [x - tx, y - ty, z - tz])
    }
}

fn apply(m: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    m.map(|row| row.iter().zip(v).map(|(a, b)| a * b).sum())
}

/// The inverse of `m` by its adjugate: each entry is a cofactor over the
/// determinant. The matrix is close to the identity, so its determinant
/// is far from zero.
fn invert(m: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let [r0, r1, r2] = m;
    let cross = |u: [f64; 3], v: [f64; 3]| [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
    // The columns of the inverse are the cross products of the rows.
    let columns = [cross(r1, r2), cross(r2, r0), cross(r0, r1)];
    let det: f64 = r0.iter().zip(columns[0]).map(|(a, b)| a * b).sum();
    let [[a0, a1, a2], [b0, b1, b2], [d0, d1, d2]] = columns;
    [[a0, b0, d0], [a1, b1, d1], [a2, b2, d2]].map(|row| row.map(|v| v / det))
}

/// One half of `x`, for the half-angle formulas of the projections.
pub(super) fn half(x: f64) -> f64 {
    x * HALF
}
