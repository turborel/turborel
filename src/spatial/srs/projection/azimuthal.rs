//! Azimuthal projections of EPSG Guidance Note 7-2: Polar Stereographic
//! variants A (9810), B (9829) and C (9830), Oblique Stereographic (9809)
//! and Lambert Azimuthal Equal Area (9820), polar and oblique aspects.
//!
//! The stereographic formulas are written through isometric latitude ψ:
//! the polar t is e^−ψ, and the oblique method's conformal sphere has
//! χ = gd(n ψ + ½ ln c), Gudermannian gd(x) = atan(sinh x), which is the
//! Guidance Note's w = c (Sa Sb^e)^n with ln Sa + e ln Sb = 2ψ.

use std::f64::consts::FRAC_PI_2;

use super::super::ellipsoid::{Ellipsoid, half};
use super::super::fault::{Fault, Outcome};
use super::super::{HALF, TWO};
use super::{Method, Params, wrap};

/// How close to a pole a latitude of origin makes an aspect polar.
const POLE: f64 = 1e-10;

/// Whether `phi` is a pole, so an aspect is polar.
pub(super) fn is_pole(phi: f64) -> bool {
    (FRAC_PI_2 - phi.abs()).abs() < POLE
}

/// +1 for the north pole, −1 for the south, of latitude `phi`.
fn hemisphere(phi: f64) -> f64 {
    1.0_f64.copysign(phi)
}

/// Polar Stereographic: ρ = scale·t, t = e^∓ψ.
#[derive(Clone, Debug, PartialEq)]
pub struct Polar {
    ellipsoid: Ellipsoid,
    params: Params,
    /// +1 north, −1 south.
    sign: f64,
    scale: f64,
    /// The pole's northing.
    pole: f64,
}

impl Polar {
    /// Variant A takes the pole as latitude of origin and a scale factor;
    /// B and C a standard parallel φc, the first standard parallel or else
    /// the latitude of origin, with ρ = a mc t/tc. C puts the false origin
    /// where φc meets the longitude of origin, ρF = a mc from the pole.
    ///
    /// # Errors
    ///
    /// F15 failed to transform geometry if variant A's origin is not a
    /// pole or a standard parallel is the equator.
    pub fn new(e: &Ellipsoid, method: Method, params: &Params) -> Outcome<Polar> {
        let invalid = || Fault::transform("[ISO/IEC 13249-3:2016 §14.1.9 Description 1i] a polar stereographic origin is invalid");
        let ce = (((1.0_f64 + e.e) * e.e.ln_1p() + (1.0_f64 - e.e) * (-e.e).ln_1p()) * HALF).exp();
        let (sign, scale, offset) = if method == Method::PolarA {
            if !is_pole(params.lat0) {
                return Err(invalid());
            }
            (hemisphere(params.lat0), TWO * e.a * params.k0 / ce, 0.0_f64)
        } else {
            let phic = params.parallel1.unwrap_or(params.lat0);
            if phic.abs() < POLE {
                return Err(invalid());
            }
            let sign = hemisphere(phic);
            let mc = e.m(phic);
            let scale = if is_pole(phic) { TWO * e.a / ce } else { e.a * mc / (-sign * e.isometric(phic)).exp() };
            let offset = if method == Method::PolarC { sign * e.a * mc } else { 0.0_f64 };
            (sign, scale, offset)
        };
        Ok(Polar { ellipsoid: *e, params: *params, sign, scale, pole: params.northing + offset })
    }

    /// E = FE + ρ sin θ, N = N(pole) ∓ ρ cos θ.
    pub fn forward(&self, lambda: f64, phi: f64) -> [f64; 2] {
        let rho = self.scale * (-self.sign * self.ellipsoid.isometric(phi)).exp();
        let (s, c) = wrap(lambda - self.params.lon0).sin_cos();
        [self.params.easting + rho * s, self.pole - self.sign * rho * c]
    }

    /// ψ = ∓ln(ρ/scale), θ = atan2(ΔE, ∓ΔN).
    pub fn inverse(&self, x: f64, y: f64) -> [f64; 2] {
        let (dx, dy) = (x - self.params.easting, y - self.pole);
        let psi = -self.sign * (dx.hypot(dy) / self.scale).ln();
        [wrap(self.params.lon0 + dx.atan2(-self.sign * dy)), self.ellipsoid.from_isometric(psi)]
    }
}

/// Oblique Stereographic, the double projection through a conformal
/// sphere of radius R = √(ρ0 ν0).
#[derive(Clone, Debug, PartialEq)]
pub struct Oblique {
    ellipsoid: Ellipsoid,
    params: Params,
    n: f64,
    /// ½ ln c.
    shift: f64,
    chi0: f64,
    /// 2 R k0.
    diameter: f64,
}

impl Oblique {
    /// The constants R, n, c and χ0 of the Guidance Note.
    pub fn new(e: &Ellipsoid, params: &Params) -> Oblique {
        let s0 = params.lat0.sin();
        let w = 1.0_f64 - e.e2 * s0 * s0;
        let radius = e.a * (1.0_f64 - e.e2).sqrt() / w;
        let c0 = params.lat0.cos();
        let n = (1.0_f64 + e.e2 * c0 * c0 * c0 * c0 / (1.0_f64 - e.e2)).sqrt();
        let psi0 = e.isometric(params.lat0);
        let sin_chi = (n * psi0).tanh();
        let c = (n + s0) * (1.0_f64 - sin_chi) / ((n - s0) * (1.0_f64 + sin_chi));
        let shift = half(c.ln());
        let chi0 = (n * psi0 + shift).sinh().atan();
        Oblique { ellipsoid: *e, params: *params, n, shift, chi0, diameter: TWO * radius * params.k0 }
    }

    /// Stereographic projection of the conformal sphere about χ0.
    pub fn forward(&self, lambda: f64, phi: f64) -> [f64; 2] {
        let p = &self.params;
        let chi = (self.n * self.ellipsoid.isometric(phi) + self.shift).sinh().atan();
        let (sl, cl) = (self.n * wrap(lambda - p.lon0)).sin_cos();
        let (s, c) = chi.sin_cos();
        let (s0, c0) = self.chi0.sin_cos();
        let b = 1.0_f64 + s * s0 + c * c0 * cl;
        [p.easting + self.diameter * c * sl / b, p.northing + self.diameter * (s * c0 - c * s0 * cl) / b]
    }

    /// The inverse stereographic projection (Snyder 1987, eqs. 20-14,
    /// 21-15), ρ = tan(c/2) of the scaled offsets, then ψ and φ.
    pub fn inverse(&self, x: f64, y: f64) -> [f64; 2] {
        let p = &self.params;
        let (dx, dy) = ((x - p.easting) / self.diameter, (y - p.northing) / self.diameter);
        let rho = dx.hypot(dy);
        let (s0, c0) = self.chi0.sin_cos();
        let (chi, big_lambda) = if rho > 0.0_f64 {
            let (sc, cc) = (TWO * rho.atan()).sin_cos();
            ((cc * s0 + dy * sc * c0 / rho).asin(), (dx * sc).atan2(rho * c0 * cc - dy * s0 * sc))
        } else {
            (self.chi0, 0.0_f64)
        };
        let psi = (chi.tan().asinh() - self.shift) / self.n;
        [wrap(p.lon0 + big_lambda / self.n), self.ellipsoid.from_isometric(psi)]
    }
}

/// Lambert Azimuthal Equal Area through authalic latitude β = asin(q/qP).
#[derive(Clone, Debug, PartialEq)]
pub struct LambertAzimuthal {
    ellipsoid: Ellipsoid,
    params: Params,
    qp: f64,
    /// Rq = a √(qP/2), the authalic sphere's radius.
    rq: f64,
    d: f64,
    beta0: f64,
    /// +1 or −1 for a polar aspect, none for an oblique one.
    polar: Option<f64>,
}

impl LambertAzimuthal {
    /// The constants qP, Rq, D and β0 of the Guidance Note.
    pub fn new(e: &Ellipsoid, params: &Params) -> LambertAzimuthal {
        let qp = e.q(FRAC_PI_2);
        let beta0 = (e.q(params.lat0) / qp).clamp(-1.0_f64, 1.0_f64).asin();
        let rq = e.a * half(qp).sqrt();
        let polar = is_pole(params.lat0).then(|| hemisphere(params.lat0));
        let d = e.a * e.m(params.lat0) / (rq * beta0.cos());
        LambertAzimuthal { ellipsoid: *e, params: *params, qp, rq, d, beta0, polar }
    }

    fn beta(&self, phi: f64) -> f64 {
        (self.ellipsoid.q(phi) / self.qp).clamp(-1.0_f64, 1.0_f64).asin()
    }

    /// The oblique formulas, or ρ = a √(qP ∓ q) at a pole.
    pub fn forward(&self, lambda: f64, phi: f64) -> [f64; 2] {
        let p = &self.params;
        let (sl, cl) = wrap(lambda - p.lon0).sin_cos();
        if let Some(sign) = self.polar {
            let rho = self.ellipsoid.a * (self.qp - sign * self.ellipsoid.q(phi)).max(0.0_f64).sqrt();
            return [p.easting + rho * sl, p.northing - sign * rho * cl];
        }
        let (sb, cb) = self.beta(phi).sin_cos();
        let (s0, c0) = self.beta0.sin_cos();
        let b = self.rq * (TWO / (1.0_f64 + s0 * sb + c0 * cb * cl)).sqrt();
        [p.easting + b * self.d * cb * sl, p.northing + b / self.d * (c0 * sb - s0 * cb * cl)]
    }

    /// The inverse, through C = 2 asin(ρ/(2 Rq)) when oblique.
    pub fn inverse(&self, x: f64, y: f64) -> [f64; 2] {
        let p = &self.params;
        let (dx, dy) = (x - p.easting, y - p.northing);
        if let Some(sign) = self.polar {
            let rho = dx.hypot(dy) / self.ellipsoid.a;
            let q = sign * (self.qp - rho * rho);
            return [wrap(p.lon0 + dx.atan2(-sign * dy)), self.ellipsoid.from_q(q)];
        }
        let rho = (dx / self.d).hypot(self.d * dy);
        if rho <= 0.0_f64 {
            return [p.lon0, p.lat0];
        }
        let (sc, cc) = (TWO * (rho / (TWO * self.rq)).clamp(-1.0_f64, 1.0_f64).asin()).sin_cos();
        let (s0, c0) = self.beta0.sin_cos();
        let beta = (cc * s0 + self.d * dy * sc * c0 / rho).clamp(-1.0_f64, 1.0_f64).asin();
        let delta = (dx * sc).atan2(self.d * rho * c0 * cc - self.d * self.d * dy * s0 * sc);
        [wrap(p.lon0 + delta), self.ellipsoid.from_q(self.qp * beta.sin())]
    }
}
