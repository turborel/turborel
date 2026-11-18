//! Conic projections of EPSG Guidance Note 7-2: Lambert Conic Conformal
//! with one standard parallel (9801) or two (9802), and Albers Equal Area
//! (9822). A cone with its apex at the south pole has a negative cone
//! constant n, which flips the radius and angle of the inverse formulas.

use super::super::ellipsoid::Ellipsoid;
use super::super::fault::{Fault, Outcome};
use super::{Method, Params, wrap};

/// The cone constant below which the cone is a cylinder.
const FLAT_CONE: f64 = 1e-10;

fn degenerate() -> Fault {
    Fault::transform("[ISO/IEC 13249-3:2016 §14.1.9 Description 1i] the standard parallels define no cone")
}

fn missing() -> Fault {
    Fault::transform("[ISO/IEC 13249-3:2016 §14.1.9 Description 1i] a conic projection lacks its standard parallel")
}

/// The polar angle θ' and signed radius r' of easting offset `dx` and
/// northing offset from the apex `dy` on a cone of constant `n`.
fn polar(n: f64, dx: f64, dy: f64) -> (f64, f64) {
    let sign = 1.0_f64.copysign(n);
    ((sign * dx).atan2(sign * dy), sign * dx.hypot(dy))
}

/// Lambert Conic Conformal: r = a F k0 t^n with t = e^−ψ.
#[derive(Clone, Debug, PartialEq)]
pub struct LambertConic {
    ellipsoid: Ellipsoid,
    params: Params,
    n: f64,
    /// a·F·k0.
    scale: f64,
    /// The radius at the latitude of the (false) origin.
    origin: f64,
}

impl LambertConic {
    /// 1SP: n = sin φ0 and the origin's scale factor k0. 2SP: n from the
    /// two standard parallels, the second defaulting to the first, and
    /// the false origin at the latitude of origin.
    ///
    /// # Errors
    ///
    /// F15 failed to transform geometry if a 2SP standard parallel is
    /// missing or the cone constant is zero.
    pub fn new(e: &Ellipsoid, method: Method, params: &Params) -> Outcome<LambertConic> {
        let (phi1, phi2, k0) = if method == Method::LambertConic1 {
            (params.lat0, params.lat0, params.k0)
        } else {
            let phi1 = params.parallel1.ok_or_else(missing)?;
            (phi1, params.parallel2.unwrap_or(phi1), 1.0_f64)
        };
        let (iso1, iso2) = (e.isometric(phi1), e.isometric(phi2));
        let n = if (phi1 - phi2).abs() < FLAT_CONE { phi1.sin() } else { (e.m(phi1).ln() - e.m(phi2).ln()) / (iso2 - iso1) };
        if n.abs() < FLAT_CONE {
            return Err(degenerate());
        }
        let scale = e.a * k0 * e.m(phi1) / (n * (-n * iso1).exp());
        let origin = scale * (-n * e.isometric(params.lat0)).exp();
        Ok(LambertConic { ellipsoid: *e, params: *params, n, scale, origin })
    }

    /// E = FE + r sin θ, N = FN + r0 − r cos θ, θ = n(λ − λ0).
    pub fn forward(&self, lambda: f64, phi: f64) -> [f64; 2] {
        let p = &self.params;
        let r = self.scale * (-self.n * self.ellipsoid.isometric(phi)).exp();
        let (s, c) = (self.n * wrap(lambda - p.lon0)).sin_cos();
        [p.easting + r * s, p.northing + self.origin - r * c]
    }

    /// ψ = −ln(r'/(a F k0))/n.
    pub fn inverse(&self, x: f64, y: f64) -> [f64; 2] {
        let p = &self.params;
        let (theta, r) = polar(self.n, x - p.easting, self.origin - (y - p.northing));
        let psi = -(r / self.scale).ln() / self.n;
        [wrap(p.lon0 + theta / self.n), self.ellipsoid.from_isometric(psi)]
    }
}

/// Albers Equal Area: ρ = a √(C − n q)/n.
#[derive(Clone, Debug, PartialEq)]
pub struct Albers {
    ellipsoid: Ellipsoid,
    params: Params,
    n: f64,
    c: f64,
    /// ρ at the latitude of the false origin.
    origin: f64,
}

impl Albers {
    /// n = (m1² − m2²)/(q2 − q1), C = m1² + n q1; one parallel, or two
    /// equal ones, give n = sin φ1.
    ///
    /// # Errors
    ///
    /// F15 failed to transform geometry if the first standard parallel is
    /// missing or the cone constant is zero.
    pub fn new(e: &Ellipsoid, params: &Params) -> Outcome<Albers> {
        let phi1 = params.parallel1.ok_or_else(missing)?;
        let phi2 = params.parallel2.unwrap_or(phi1);
        let (m1, m2) = (e.m(phi1), e.m(phi2));
        let (q1, q2) = (e.q(phi1), e.q(phi2));
        let n = if (phi1 - phi2).abs() < FLAT_CONE { phi1.sin() } else { (m1 * m1 - m2 * m2) / (q2 - q1) };
        if n.abs() < FLAT_CONE {
            return Err(degenerate());
        }
        let c = m1 * m1 + n * q1;
        let mut albers = Albers { ellipsoid: *e, params: *params, n, c, origin: 0.0_f64 };
        albers.origin = albers.rho(params.lat0);
        Ok(albers)
    }

    fn rho(&self, phi: f64) -> f64 {
        self.ellipsoid.a * (self.c - self.n * self.ellipsoid.q(phi)).sqrt() / self.n
    }

    /// E = EF + ρ sin θ, N = NF + ρ0 − ρ cos θ, θ = n(λ − λ0).
    pub fn forward(&self, lambda: f64, phi: f64) -> [f64; 2] {
        let p = &self.params;
        let rho = self.rho(phi);
        let (s, c) = (self.n * wrap(lambda - p.lon0)).sin_cos();
        [p.easting + rho * s, p.northing + self.origin - rho * c]
    }

    /// q' = (C − ρ'² n²/a²)/n, then the latitude of q'.
    pub fn inverse(&self, x: f64, y: f64) -> [f64; 2] {
        let p = &self.params;
        let (theta, rho) = polar(self.n, x - p.easting, self.origin - (y - p.northing));
        let scaled = rho * self.n / self.ellipsoid.a;
        let q = (self.c - scaled * scaled) / self.n;
        [wrap(p.lon0 + theta / self.n), self.ellipsoid.from_q(q)]
    }
}
