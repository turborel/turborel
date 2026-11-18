//! The direct geodesic problem, Karney 2013 §2 and §6: a geodesic from a
//! point and azimuth, and the point at any distance along it, with the
//! reduced length and geodesic scale the point-to-segment search uses.

use super::super::TWO;
use super::Geodesic;
use super::math::{self, DEGREE, norm, sincosd};
use super::series::{self, ORDER, sin_cos_series};
use super::solve::reduced;

/// The flattening above which a Newton step refines σ12 (Karney 2013 §6).
const ROUGH: f64 = 0.01;

/// A point along a geodesic line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Position {
    /// Longitude, degrees.
    pub lon: f64,
    /// Latitude, degrees.
    pub lat: f64,
    /// Forward azimuth there, degrees.
    pub azimuth: f64,
    /// Reduced length m12 from the start, metres.
    pub reduced: f64,
    /// Geodesic scale M12.
    pub scale12: f64,
}

/// A geodesic from a point in a direction.
#[derive(Clone, Debug)]
pub struct Line {
    lon1: f64,
    b: f64,
    f: f64,
    f1: f64,
    salp0: f64,
    calp0: f64,
    ssig1: f64,
    csig1: f64,
    somg1: f64,
    comg1: f64,
    dn1: f64,
    k2: f64,
    stau1: f64,
    ctau1: f64,
    a1m1: f64,
    a2m1: f64,
    a3c: f64,
    b11: f64,
    b21: f64,
    b31: f64,
    c1: [f64; ORDER + 1],
    c1p: [f64; ORDER + 1],
    c2: [f64; ORDER + 1],
    c3: [f64; ORDER],
}

impl Line {
    /// The geodesic from (`lat1`, `lon1`) at azimuth `azimuth`, degrees.
    pub fn new(g: &Geodesic, lat1: f64, lon1: f64, azimuth: f64) -> Line {
        let (salp1, calp1) = sincosd(math::round(azimuth));
        let p1 = reduced(g, math::latitude(lat1));
        let salp0 = salp1 * p1.c;
        let calp0 = calp1.hypot(salp1 * p1.s);
        let comg1 = if p1.s != 0.0_f64 || calp1 != 0.0_f64 { p1.c * calp1 } else { 1.0_f64 };
        let (ssig1, csig1) = norm(p1.s, comg1);
        let k2 = calp0 * calp0 * g.ep2;
        let eps = k2 / (TWO * (1.0_f64 + (1.0_f64 + k2).sqrt()) + k2);
        let c1 = series::c1(eps);
        let c2 = series::c2(eps);
        let c3 = series::c3(&g.c3x, eps);
        let b11 = sin_cos_series(true, ssig1, csig1, &c1);
        let (s, c) = b11.sin_cos();
        let a3c = -g.f * salp0 * series::a3(&g.a3x, eps);
        Line {
            lon1,
            b: g.b,
            f: g.f,
            f1: g.f1,
            salp0,
            calp0,
            ssig1,
            csig1,
            somg1: salp0 * p1.s,
            comg1,
            dn1: (1.0_f64 + k2 * ssig1 * ssig1).sqrt(),
            k2,
            stau1: ssig1 * c + csig1 * s,
            ctau1: csig1 * c - ssig1 * s,
            a1m1: series::a1m1(eps),
            a2m1: series::a2m1(eps),
            a3c,
            b11,
            b21: sin_cos_series(true, ssig1, csig1, &c2),
            b31: sin_cos_series(true, ssig1, csig1, &c3),
            c1,
            c1p: series::c1p(eps),
            c2,
            c3,
        }
    }

    /// σ12 at distance `s12` metres (Karney 2013 eq. 20).
    fn sigma(&self, s12: f64) -> f64 {
        let tau12 = s12 / (self.b * (1.0_f64 + self.a1m1));
        let (s, c) = tau12.sin_cos();
        let b12 = -sin_cos_series(true, self.stau1 * c + self.ctau1 * s, self.ctau1 * c - self.stau1 * s, &self.c1p);
        let sig12 = tau12 - (b12 - self.b11);
        if self.f.abs() <= ROUGH {
            return sig12;
        }
        let (arc_sin, arc_cos) = sig12.sin_cos();
        let end_sin = self.ssig1 * arc_cos + self.csig1 * arc_sin;
        let end_cos = self.csig1 * arc_cos - self.ssig1 * arc_sin;
        let refined = sin_cos_series(true, end_sin, end_cos, &self.c1);
        let error = (1.0_f64 + self.a1m1) * (sig12 + (refined - self.b11)) - s12 / self.b;
        sig12 - error / (1.0_f64 + self.k2 * end_sin * end_sin).sqrt()
    }

    /// The point `s12` metres along the line.
    pub fn position(&self, s12: f64) -> Position {
        let sig12 = self.sigma(s12);
        let (arc_sin, arc_cos) = sig12.sin_cos();
        let end_sin = self.ssig1 * arc_cos + self.csig1 * arc_sin;
        let mut end_cos = self.csig1 * arc_cos - self.ssig1 * arc_sin;
        let dn2 = (1.0_f64 + self.k2 * end_sin * end_sin).sqrt();
        let sbet2 = self.calp0 * end_sin;
        let mut cbet2 = self.salp0.hypot(self.calp0 * end_cos);
        if cbet2 == 0.0_f64 {
            cbet2 = f64::MIN_POSITIVE.sqrt();
            end_cos = cbet2;
        }
        let e = 1.0_f64.copysign(self.salp0);
        let omg12 = e
            * (sig12 - (end_sin.atan2(end_cos) - self.ssig1.atan2(self.csig1))
                + ((e * self.salp0 * end_sin).atan2(end_cos) - (e * self.somg1).atan2(self.comg1)));
        let lam12 = omg12 + self.a3c * (sig12 + (sin_cos_series(true, end_sin, end_cos, &self.c3) - self.b31));
        let lon2 = math::normalize(math::normalize(self.lon1) + math::normalize(lam12 / DEGREE));
        let ab1 = (1.0_f64 + self.a1m1) * (sin_cos_series(true, end_sin, end_cos, &self.c1) - self.b11);
        let ab2 = (1.0_f64 + self.a2m1) * (sin_cos_series(true, end_sin, end_cos, &self.c2) - self.b21);
        let j12 = (self.a1m1 - self.a2m1) * sig12 + (ab1 - ab2);
        let m12 = self.b * ((dn2 * (self.csig1 * end_sin) - self.dn1 * (self.ssig1 * end_cos)) - self.csig1 * end_cos * j12);
        let t = self.k2 * (end_sin - self.ssig1) * (end_sin + self.ssig1) / (self.dn1 + dn2);
        Position {
            lon: lon2,
            lat: math::atan2d(sbet2, self.f1 * cbet2),
            azimuth: math::atan2d(self.salp0, self.calp0 * end_cos),
            reduced: m12,
            scale12: arc_cos + (t * end_sin - end_cos * j12) * self.ssig1 / self.dn1,
        }
    }
}
