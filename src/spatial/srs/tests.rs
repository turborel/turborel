//! Requirements of the reference system engine, by area. Expected values
//! of the projections and conversions are the worked examples of EPSG
//! Guidance Note 7-2 and of Snyder, "Map Projections: A Working Manual"
//! (USGS Professional Paper 1395, 1987), to the precision they print.

mod angles;
mod directions;
mod geodesics;
mod limits;
mod polygons;
mod projections;
mod systems;
mod text;
mod transforms;
mod units;

use super::model::CoordinateSystem;
use super::transform::Transform;
use super::wkt;

/// Degrees of `d`° `m`′ `s`″, the sign of `d` applying to all three.
pub(super) fn dms(d: f64, m: f64, s: f64) -> f64 {
    let magnitude = d.abs() + m / 60.0_f64 + s / 3600.0_f64;
    if d.is_sign_negative() { -magnitude } else { magnitude }
}

/// The system `text` names, read or the test fails.
pub(super) fn system(text: &str) -> CoordinateSystem {
    wkt::read(text).unwrap_or_else(|fault| panic!("{fault}: {text}"))
}

/// The definition the catalog supplies for `srid`, or the test fails.
pub(super) fn seed(srid: i32) -> String {
    crate::system::SpatialRefSys::by_id(srid).map_or_else(|| panic!("no system {srid}"), |row| row.definition.clone())
}

/// The geographic system a projected system `projected` is based on.
pub(super) fn base(projected: &CoordinateSystem) -> CoordinateSystem {
    match projected {
        CoordinateSystem::Projected(cs) => CoordinateSystem::Geographic(Box::new(cs.geographic.clone())),
        CoordinateSystem::Geographic(_) | CoordinateSystem::Geocentric(_) => panic!("not projected"),
    }
}

/// Asserts `actual` is within `tolerance` of `expected`, each coordinate.
pub(super) fn near<const N: usize>(actual: [f64; N], expected: [f64; N], tolerance: f64) {
    for (a, e) in actual.iter().zip(expected) {
        assert!((a - e).abs() <= tolerance, "{actual:?} is not within {tolerance} of {expected:?}");
    }
}

/// Projects `lonlat` with `projcs` and checks the published `expected`
/// within `printed` (half the last printed digit), then the inverse of
/// `expected` within the same distance in degrees, and the round trip of
/// `lonlat` within 10⁻⁹ degree, about 0.1 mm.
pub(super) fn projects(projcs: &str, lonlat: [f64; 2], expected: [f64; 2], printed: f64) {
    let projected = system(projcs);
    let geographic = base(&projected);
    let forward = Transform::new(&geographic, &projected).unwrap();
    let inverse = Transform::new(&projected, &geographic).unwrap();
    near(forward.apply_2d(lonlat).unwrap(), expected, printed);
    let degrees = printed / 30_000.0_f64;
    near(inverse.apply_2d(expected).unwrap(), lonlat, degrees);
    near(inverse.apply_2d(forward.apply_2d(lonlat).unwrap()).unwrap(), lonlat, 1e-9_f64);
}
