//! Transverse Mercator, EPSG method 9807, by Krüger's series to sixth
//! order in the third flattening n, as Karney gives them in "Transverse
//! Mercator with an accuracy of a few nanometers" (J. Geodesy 85, 2011),
//! eqs. (35) and (36). Within 3900 km of the central meridian the error
//! is below a millimetre, far below a UTM zone's extent.
//!
//! The same series map conformal latitude to rectifying latitude, which
//! is the meridian arc length the equidistant cylindrical method needs.

use super::super::TWO;
use super::super::ellipsoid::Ellipsoid;
use super::{Params, wrap};

/// The series order.
const ORDER: usize = 6;

/// αj of Karney 2011 eq. (35): row j − 1 holds the coefficients of
/// n^1 to n^6 in αj.
const ALPHA: [[f64; ORDER]; ORDER] = [
    [1.0 / 2.0, -2.0 / 3.0, 5.0 / 16.0, 41.0 / 180.0, -127.0 / 288.0, 7891.0 / 37800.0],
    [0.0, 13.0 / 48.0, -3.0 / 5.0, 557.0 / 1440.0, 281.0 / 630.0, -1_983_433.0 / 1_935_360.0],
    [0.0, 0.0, 61.0 / 240.0, -103.0 / 140.0, 15061.0 / 26880.0, 167_603.0 / 181_440.0],
    [0.0, 0.0, 0.0, 49561.0 / 161_280.0, -179.0 / 168.0, 6_601_661.0 / 7_257_600.0],
    [0.0, 0.0, 0.0, 0.0, 34729.0 / 80640.0, -3_418_889.0 / 1_995_840.0],
    [0.0, 0.0, 0.0, 0.0, 0.0, 212_378_941.0 / 319_334_400.0],
];

/// βj of Karney 2011 eq. (36), laid out as [`ALPHA`].
const BETA: [[f64; ORDER]; ORDER] = [
    [1.0 / 2.0, -2.0 / 3.0, 37.0 / 96.0, -1.0 / 360.0, -81.0 / 512.0, 96199.0 / 604_800.0],
    [0.0, 1.0 / 48.0, 1.0 / 15.0, -437.0 / 1440.0, 46.0 / 105.0, -1_118_711.0 / 3_870_720.0],
    [0.0, 0.0, 17.0 / 480.0, -37.0 / 840.0, -209.0 / 4480.0, 5569.0 / 90720.0],
    [0.0, 0.0, 0.0, 4397.0 / 161_280.0, -11.0 / 504.0, -830_251.0 / 7_257_600.0],
    [0.0, 0.0, 0.0, 0.0, 4583.0 / 161_280.0, -108_847.0 / 3_991_680.0],
    [0.0, 0.0, 0.0, 0.0, 0.0, 20_648_693.0 / 638_668_800.0],
];

/// The rectifying radius A = a/(1 + n)·(1 + n²/4 + n⁴/64 + n⁶/256),
/// Karney 2011 eq. (14): coefficients of n², n⁴ and n⁶.
const RECTIFYING: [f64; 3] = [1.0 / 4.0, 1.0 / 64.0, 1.0 / 256.0];

/// The Krüger series of one ellipsoid.
#[derive(Clone, Debug, PartialEq)]
pub struct Krueger {
    /// The rectifying radius A.
    pub radius: f64,
    alpha: [f64; ORDER],
    beta: [f64; ORDER],
}

/// Each row of `table` evaluated as a polynomial in `n` with no constant.
fn series(table: &[[f64; ORDER]; ORDER], n: f64) -> [f64; ORDER] {
    table.map(|row| row.iter().rev().fold(0.0_f64, |sum, c| (sum + c) * n))
}

impl Krueger {
    /// The series of `e`.
    pub fn new(e: &Ellipsoid) -> Krueger {
        let n2 = e.n * e.n;
        let polynomial = RECTIFYING.iter().rev().fold(0.0_f64, |sum, c| (sum + c) * n2);
        Krueger { radius: e.a / (1.0_f64 + e.n) * (1.0_f64 + polynomial), alpha: series(&ALPHA, e.n), beta: series(&BETA, e.n) }
    }

    /// ζ + Σ cj sin(2jζ) for complex ζ = ξ + iη, as (ξ, η) parts.
    fn sum(coefficients: &[f64; ORDER], xi: f64, eta: f64, sign: f64) -> [f64; 2] {
        let mut out = [xi, eta];
        let mut j = 0.0_f64;
        for c in coefficients {
            j += 1.0_f64;
            let (sine, cosine) = (TWO * j * xi).sin_cos();
            let (hyperbolic_sine, hyperbolic_cosine) = ((TWO * j * eta).sinh(), (TWO * j * eta).cosh());
            out[0] += sign * c * sine * hyperbolic_cosine;
            out[1] += sign * c * cosine * hyperbolic_sine;
        }
        out
    }

    /// Rectifying latitude μ of conformal latitude `chi`.
    pub fn rectifying(&self, chi: f64) -> f64 {
        Self::sum(&self.alpha, chi, 0.0_f64, 1.0_f64)[0]
    }

    /// Conformal latitude χ of rectifying latitude `mu`.
    pub fn conformal(&self, mu: f64) -> f64 {
        Self::sum(&self.beta, mu, 0.0_f64, -1.0_f64)[0]
    }
}

/// Transverse Mercator prepared for one ellipsoid and parameter set.
#[derive(Clone, Debug, PartialEq)]
pub struct TransverseMercator {
    ellipsoid: Ellipsoid,
    krueger: Krueger,
    params: Params,
    /// k0·A·ξ0, the scaled meridian arc to the latitude of origin.
    origin: f64,
}

impl TransverseMercator {
    /// The projection of `params` on `e`.
    pub fn new(e: &Ellipsoid, params: &Params) -> TransverseMercator {
        let krueger = Krueger::new(e);
        let origin = params.k0 * krueger.radius * krueger.rectifying(e.conformal(params.lat0));
        TransverseMercator { ellipsoid: *e, krueger, params: *params, origin }
    }

    /// Karney 2011 eqs. (7) to (11): ξ' and η' of the conformal sphere,
    /// then the Krüger series.
    pub fn forward(&self, lambda: f64, phi: f64) -> [f64; 2] {
        let p = &self.params;
        let (sl, cl) = wrap(lambda - p.lon0).sin_cos();
        let taup = self.ellipsoid.conformal_tan(phi.tan());
        let xip = taup.atan2(cl);
        let etap = (sl / taup.hypot(cl)).asinh();
        let [xi, eta] = Krueger::sum(&self.krueger.alpha, xip, etap, 1.0_f64);
        let scale = p.k0 * self.krueger.radius;
        [p.easting + scale * eta, p.northing + scale * xi - self.origin]
    }

    /// The inverse series, then τ' and the geodetic latitude.
    pub fn inverse(&self, x: f64, y: f64) -> [f64; 2] {
        let p = &self.params;
        let scale = p.k0 * self.krueger.radius;
        let xi = (y - p.northing + self.origin) / scale;
        let eta = (x - p.easting) / scale;
        let [xip, etap] = Krueger::sum(&self.krueger.beta, xi, eta, -1.0_f64);
        let (sin_xi, cos_xi) = xip.sin_cos();
        let sinh_eta = etap.sinh();
        let taup = sin_xi / sinh_eta.hypot(cos_xi);
        let phi = self.ellipsoid.geodetic_tan(taup).atan();
        [wrap(p.lon0 + sinh_eta.atan2(cos_xi)), phi]
    }
}
