//! Map projections of a \<projected cs\> (ISO/IEC 13249-3:2016 (DIS, SC32
//! N2593) §14.1.9 Description 1d and 1i). Projection and parameter names
//! are implementation-defined; this module accepts the names of the OGC
//! coordinate transformation text and of the EPSG dataset, compared
//! without regard to case, spaces or underscores, and implements each
//! method as EPSG Guidance Note 7-2 (2019) defines it, forward and
//! inverse.
//!
//! Angular parameters are in the geographic system's angular unit and
//! relative to its prime meridian; false eastings and northings are in
//! the projected system's linear unit. Missing parameters take the
//! values of an unprojected origin: zero, and a scale factor of one.

mod azimuthal;
mod conic;
mod cylindrical;
mod tmerc;

use std::f64::consts::{PI, TAU};

use super::ellipsoid::Ellipsoid;
use super::fault::{Fault, Outcome};
use super::model::Parameter;

/// A projection method of EPSG Guidance Note 7-2, with its EPSG code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// Transverse Mercator, 9807, with UTM as its zones.
    TransverseMercator,
    /// Mercator (variant A), 9804, the OGC Mercator_1SP.
    MercatorA,
    /// Mercator (variant B), 9805, the OGC Mercator_2SP.
    MercatorB,
    /// Popular Visualisation Pseudo Mercator, 1024.
    PseudoMercator,
    /// Lambert Conic Conformal (1SP), 9801.
    LambertConic1,
    /// Lambert Conic Conformal (2SP), 9802.
    LambertConic2,
    /// Albers Equal Area, 9822.
    Albers,
    /// Polar Stereographic (variant A), 9810.
    PolarA,
    /// Polar Stereographic (variant B), 9829.
    PolarB,
    /// Polar Stereographic (variant C), 9830.
    PolarC,
    /// Oblique Stereographic, 9809.
    ObliqueStereographic,
    /// Lambert Azimuthal Equal Area, 9820.
    LambertAzimuthal,
    /// Equidistant Cylindrical, 1028; on a sphere, Equirectangular.
    EquidistantCylindrical,
    /// Equidistant Cylindrical (Spherical), 1029.
    EquidistantSpherical,
}

/// The parameter names, normalized, of the OGC text and the EPSG dataset.
const ROLES: [(&str, Role); 26] = [
    ("false_easting", Role::Easting),
    ("easting_at_false_origin", Role::Easting),
    ("false_northing", Role::Northing),
    ("northing_at_false_origin", Role::Northing),
    ("central_meridian", Role::Longitude),
    ("longitude_of_origin", Role::Longitude),
    ("longitude_of_center", Role::Longitude),
    ("longitude_of_centre", Role::Longitude),
    ("longitude_of_natural_origin", Role::Longitude),
    ("longitude_of_false_origin", Role::Longitude),
    ("straight_vertical_longitude_from_pole", Role::Longitude),
    ("latitude_of_origin", Role::Latitude),
    ("latitude_of_center", Role::Latitude),
    ("latitude_of_centre", Role::Latitude),
    ("latitude_of_natural_origin", Role::Latitude),
    ("latitude_of_false_origin", Role::Latitude),
    ("scale_factor", Role::Scale),
    ("scale_factor_at_natural_origin", Role::Scale),
    ("standard_parallel_1", Role::Parallel1),
    ("latitude_of_1st_standard_parallel", Role::Parallel1),
    ("latitude_of_standard_parallel", Role::Parallel1),
    ("standard_parallel_2", Role::Parallel2),
    ("latitude_of_2nd_standard_parallel", Role::Parallel2),
    ("pseudo_standard_parallel_1", Role::Parallel1),
    ("standard_parallel", Role::Parallel1),
    ("k0", Role::Scale),
];

/// The projection names, normalized, of the OGC text and the EPSG dataset.
/// A generic polar stereographic name picks its variant from parameters.
const NAMES: [(&str, Option<Method>); 27] = [
    ("transverse_mercator", Some(Method::TransverseMercator)),
    ("gauss_kruger", Some(Method::TransverseMercator)),
    ("mercator", Some(Method::MercatorA)),
    ("mercator_1sp", Some(Method::MercatorA)),
    ("mercator_variant_a", Some(Method::MercatorA)),
    ("mercator_2sp", Some(Method::MercatorB)),
    ("mercator_variant_b", Some(Method::MercatorB)),
    ("popular_visualisation_pseudo_mercator", Some(Method::PseudoMercator)),
    ("pseudo_mercator", Some(Method::PseudoMercator)),
    ("lambert_conformal_conic_1sp", Some(Method::LambertConic1)),
    ("lambert_conic_conformal_1sp", Some(Method::LambertConic1)),
    ("lambert_conformal_conic", Some(Method::LambertConic2)),
    ("lambert_conformal_conic_2sp", Some(Method::LambertConic2)),
    ("lambert_conic_conformal_2sp", Some(Method::LambertConic2)),
    ("albers_conic_equal_area", Some(Method::Albers)),
    ("albers_equal_area", Some(Method::Albers)),
    ("polar_stereographic", None),
    ("polar_stereographic_variant_a", Some(Method::PolarA)),
    ("polar_stereographic_variant_b", Some(Method::PolarB)),
    ("polar_stereographic_variant_c", Some(Method::PolarC)),
    ("oblique_stereographic", Some(Method::ObliqueStereographic)),
    ("double_stereographic", Some(Method::ObliqueStereographic)),
    ("lambert_azimuthal_equal_area", Some(Method::LambertAzimuthal)),
    ("equirectangular", Some(Method::EquidistantCylindrical)),
    ("equidistant_cylindrical", Some(Method::EquidistantCylindrical)),
    ("plate_carree", Some(Method::EquidistantCylindrical)),
    ("equidistant_cylindrical_spherical", Some(Method::EquidistantSpherical)),
];

/// What a parameter defines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Easting,
    Northing,
    Longitude,
    Latitude,
    Scale,
    Parallel1,
    Parallel2,
}

/// The defining parameters of a projection, in metres and radians.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Params {
    /// False easting, metres.
    pub easting: f64,
    /// False northing, metres.
    pub northing: f64,
    /// Longitude of origin, radians from the prime meridian.
    pub lon0: f64,
    /// Latitude of origin, radians.
    pub lat0: f64,
    /// Scale factor at the origin.
    pub k0: f64,
    /// First standard parallel, radians, if given.
    pub parallel1: Option<f64>,
    /// Second standard parallel, radians, if given.
    pub parallel2: Option<f64>,
}

/// `name` in lower case with each run of spaces, hyphens and underscores
/// one underscore, so "Transverse Mercator" is "transverse_mercator".
pub(crate) fn normalize(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_end_matches('_').to_owned()
}

/// `delta` reduced to [−π, π), the longitude difference of the formulas.
pub(crate) fn wrap(delta: f64) -> f64 {
    (delta + PI).rem_euclid(TAU) - PI
}

/// The parameters `parameters`, angles scaled by `angular` radians per
/// unit and lengths by `linear` metres per unit.
fn params(parameters: &[Parameter], angular: f64, linear: f64) -> Outcome<Params> {
    let mut out = Params { k0: 1.0_f64, ..Params::default() };
    for parameter in parameters {
        let key = normalize(&parameter.name);
        let role = ROLES.iter().find(|(name, _)| *name == key).map(|(_, role)| *role).ok_or_else(|| {
            Fault::transform(format!("[ISO/IEC 13249-3:2016 §14.1.9 Description 1i] the parameter {} is not supported", parameter.name))
        })?;
        let value = parameter.value;
        match role {
            Role::Easting => out.easting = value * linear,
            Role::Northing => out.northing = value * linear,
            Role::Longitude => out.lon0 = value * angular,
            Role::Latitude => out.lat0 = value * angular,
            Role::Scale => out.k0 = value,
            Role::Parallel1 => out.parallel1 = Some(value * angular),
            Role::Parallel2 => out.parallel2 = Some(value * angular),
        }
    }
    Ok(out)
}

/// The method `name` names, given the resolved parameters.
fn method(name: &str, params: &Params) -> Outcome<Method> {
    let key = normalize(name);
    let found = NAMES
        .iter()
        .find(|(known, _)| *known == key)
        .ok_or_else(|| Fault::transform(format!("[ISO/IEC 13249-3:2016 §14.1.9 Description 1d] the projection {name} is not supported")))?;
    Ok(found.1.unwrap_or_else(|| polar_variant(params)))
}

/// The OGC Polar_Stereographic: variant A if its latitude of origin is a
/// pole, else variant B with that latitude as standard parallel.
fn polar_variant(params: &Params) -> Method {
    if params.parallel1.is_some() || !azimuthal::is_pole(params.lat0) { Method::PolarB } else { Method::PolarA }
}

/// A projection prepared for one ellipsoid and parameter set.
#[derive(Clone, Debug, PartialEq)]
pub enum Projector {
    /// Transverse Mercator.
    TransverseMercator(tmerc::TransverseMercator),
    /// Mercator variants A and B and Pseudo Mercator.
    Mercator(cylindrical::Mercator),
    /// Equidistant Cylindrical, ellipsoidal or spherical.
    Equidistant(cylindrical::Equidistant),
    /// Lambert Conic Conformal, one or two standard parallels.
    LambertConic(conic::LambertConic),
    /// Albers Equal Area.
    Albers(conic::Albers),
    /// Polar Stereographic variants A, B and C.
    Polar(azimuthal::Polar),
    /// Oblique Stereographic.
    Oblique(azimuthal::Oblique),
    /// Lambert Azimuthal Equal Area.
    LambertAzimuthal(azimuthal::LambertAzimuthal),
}

/// The method `name` names and its parameters in metres and radians, by
/// which two projected systems are compared whatever names and units
/// they spell them in.
///
/// # Errors
///
/// F15 if the projection or a parameter is unsupported.
pub fn resolve(name: &str, parameters: &[Parameter], angular: f64, linear: f64) -> Outcome<(Method, Params)> {
    let params = params(parameters, angular, linear)?;
    Ok((method(name, &params)?, params))
}

impl Projector {
    /// The projection `name` with `parameters` on `ellipsoid`; angles are
    /// `angular` radians per unit, lengths `linear` metres per unit.
    ///
    /// # Errors
    ///
    /// F15 failed to transform geometry if the projection or a parameter
    /// is unsupported, or the parameters define no projection.
    pub fn new(ellipsoid: &Ellipsoid, name: &str, parameters: &[Parameter], angular: f64, linear: f64) -> Outcome<Projector> {
        let params = params(parameters, angular, linear)?;
        let method = method(name, &params)?;
        Self::prepare(ellipsoid, method, &params)
    }

    fn prepare(e: &Ellipsoid, method: Method, p: &Params) -> Outcome<Projector> {
        Ok(match method {
            Method::TransverseMercator => Projector::TransverseMercator(tmerc::TransverseMercator::new(e, p)),
            Method::MercatorA | Method::MercatorB | Method::PseudoMercator => {
                Projector::Mercator(cylindrical::Mercator::new(e, method, p)?)
            }
            Method::EquidistantCylindrical | Method::EquidistantSpherical => {
                Projector::Equidistant(cylindrical::Equidistant::new(e, method, p))
            }
            Method::LambertConic1 | Method::LambertConic2 => Projector::LambertConic(conic::LambertConic::new(e, method, p)?),
            Method::Albers => Projector::Albers(conic::Albers::new(e, p)?),
            Method::PolarA | Method::PolarB | Method::PolarC => Projector::Polar(azimuthal::Polar::new(e, method, p)?),
            Method::ObliqueStereographic => Projector::Oblique(azimuthal::Oblique::new(e, p)),
            Method::LambertAzimuthal => Projector::LambertAzimuthal(azimuthal::LambertAzimuthal::new(e, p)),
        })
    }

    /// Easting and northing, metres, of longitude `lambda` (from the prime
    /// meridian) and latitude `phi`, radians.
    pub fn forward(&self, lambda: f64, phi: f64) -> [f64; 2] {
        match self {
            Projector::TransverseMercator(p) => p.forward(lambda, phi),
            Projector::Mercator(p) => p.forward(lambda, phi),
            Projector::Equidistant(p) => p.forward(lambda, phi),
            Projector::LambertConic(p) => p.forward(lambda, phi),
            Projector::Albers(p) => p.forward(lambda, phi),
            Projector::Polar(p) => p.forward(lambda, phi),
            Projector::Oblique(p) => p.forward(lambda, phi),
            Projector::LambertAzimuthal(p) => p.forward(lambda, phi),
        }
    }

    /// Longitude (from the prime meridian) and latitude, radians, of
    /// easting `x` and northing `y`, metres.
    pub fn inverse(&self, x: f64, y: f64) -> [f64; 2] {
        match self {
            Projector::TransverseMercator(p) => p.inverse(x, y),
            Projector::Mercator(p) => p.inverse(x, y),
            Projector::Equidistant(p) => p.inverse(x, y),
            Projector::LambertConic(p) => p.inverse(x, y),
            Projector::Albers(p) => p.inverse(x, y),
            Projector::Polar(p) => p.inverse(x, y),
            Projector::Oblique(p) => p.inverse(x, y),
            Projector::LambertAzimuthal(p) => p.inverse(x, y),
        }
    }
}
