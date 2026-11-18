//! Normal cylindrical projections of EPSG Guidance Note 7-2: Mercator
//! variants A (9804) and B (9805), Popular Visualisation Pseudo Mercator
//! (1024), and Equidistant Cylindrical, ellipsoidal (1028) and spherical
//! (1029).

use std::f64::consts::FRAC_PI_2;

use super::super::ellipsoid::Ellipsoid;
use super::super::fault::{Fault, Outcome};
use super::tmerc::Krueger;
use super::{Method, Params, wrap};

/// Mercator, its northing a scaled isometric latitude.
#[derive(Clone, Debug, PartialEq)]
pub struct Mercator {
    ellipsoid: Ellipsoid,
    params: Params,
    /// a·k0, metres per radian of longitude at the equator.
    scale: f64,
    /// Pseudo Mercator takes ellipsoidal latitude as spherical.
    spherical: bool,
}

impl Mercator {
    /// Variant A takes its scale factor at the equator; variant B its
    /// standard parallel φ1, k0 = cos φ1/√(1 − e² sin² φ1); Pseudo
    /// Mercator neither. The origin is on the equator.
    ///
    /// # Errors
    ///
    /// F15 failed to transform geometry if the latitude of origin is not
    /// zero, which no Mercator variant defines.
    pub fn new(e: &Ellipsoid, method: Method, params: &Params) -> Outcome<Mercator> {
        if params.lat0.abs() > 0.0_f64 {
            return Err(Fault::transform("[ISO/IEC 13249-3:2016 §14.1.9 Description 1i] a Mercator latitude of origin is not zero"));
        }
        let k0 = if method == Method::MercatorB { e.m(params.parallel1.unwrap_or_default()) } else { params.k0 };
        Ok(Mercator { ellipsoid: *e, params: *params, scale: e.a * k0, spherical: method == Method::PseudoMercator })
    }

    /// E = FE + a k0 (λ − λ0), N = FN + a k0 ψ.
    pub fn forward(&self, lambda: f64, phi: f64) -> [f64; 2] {
        let p = &self.params;
        // The poles map to infinity, outside the projection's domain.
        let psi = if phi.abs() >= FRAC_PI_2 {
            f64::NAN
        } else if self.spherical {
            phi.tan().asinh()
        } else {
            self.ellipsoid.isometric(phi)
        };
        [p.easting + self.scale * wrap(lambda - p.lon0), p.northing + self.scale * psi]
    }

    /// The latitude of isometric latitude (N − FN)/(a k0).
    pub fn inverse(&self, x: f64, y: f64) -> [f64; 2] {
        let p = &self.params;
        let psi = (y - p.northing) / self.scale;
        let phi = if self.spherical { psi.sinh().atan() } else { self.ellipsoid.from_isometric(psi) };
        [wrap(p.lon0 + (x - p.easting) / self.scale), phi]
    }
}

/// Equidistant Cylindrical: northing the meridian arc from the equator,
/// easting scaled to be true along the standard parallel φ1.
#[derive(Clone, Debug, PartialEq)]
pub struct Equidistant {
    ellipsoid: Ellipsoid,
    krueger: Krueger,
    params: Params,
    /// ν1 cos φ1, metres per radian of longitude.
    parallel: f64,
    /// Method 1029 measures the meridian on the sphere of radius a.
    spherical: bool,
}

impl Equidistant {
    /// The projection of `params` on `e`. The standard parallel is
    /// the first standard parallel if given, else the latitude of origin.
    pub fn new(e: &Ellipsoid, method: Method, params: &Params) -> Equidistant {
        let spherical = method == Method::EquidistantSpherical;
        let phi1 = params.parallel1.unwrap_or(params.lat0);
        let parallel = if spherical { e.a * phi1.cos() } else { e.nu(phi1) * phi1.cos() };
        Equidistant { ellipsoid: *e, krueger: Krueger::new(e), params: *params, parallel, spherical }
    }

    /// The meridian arc to `phi`: A μ, μ the rectifying latitude.
    fn arc(&self, phi: f64) -> f64 {
        if self.spherical {
            return self.ellipsoid.a * phi;
        }
        self.krueger.radius * self.krueger.rectifying(self.ellipsoid.conformal(phi))
    }

    /// E = FE + ν1 cos φ1 (λ − λ0), N = FN + M.
    pub fn forward(&self, lambda: f64, phi: f64) -> [f64; 2] {
        let p = &self.params;
        [p.easting + self.parallel * wrap(lambda - p.lon0), p.northing + self.arc(phi)]
    }

    /// The latitude whose meridian arc is N − FN.
    pub fn inverse(&self, x: f64, y: f64) -> [f64; 2] {
        let p = &self.params;
        let arc = y - p.northing;
        let phi = if self.spherical {
            arc / self.ellipsoid.a
        } else {
            self.ellipsoid.from_conformal(self.krueger.conformal(arc / self.krueger.radius))
        };
        [wrap(p.lon0 + (x - p.easting) / self.parallel), phi]
    }
}
