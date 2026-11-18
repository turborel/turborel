//! Requirements of the transform pipeline of ST_Transform: geographic and
//! geocentric conversion and the Helmert shift against the worked
//! examples of EPSG Guidance Note 7-2, axis order, prime meridians, units,
//! and the failures F15 covers.

use super::super::Condition;
use super::super::ellipsoid::{Ellipsoid, Shift};
use super::super::model::Helmert;
use super::super::transform::Transform;
use super::{dms, near, system};

const WGS84: &str = r#"GEOGCS["WGS 84",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]]"#;
const WGS84_ECEF: &str =
    r#"GEOCCS["WGS 84",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563]],PRIMEM["Greenwich",0],UNIT["metre",1]]"#;
const WGS72_ECEF: &str = r#"GEOCCS["WGS 72",DATUM["WGS_1972",SPHEROID["WGS 72",6378135,298.26],TOWGS84[0,0,4.5,0,0,0.554,0.219]],PRIMEM["Greenwich",0],UNIT["metre",1]]"#;

fn transform(from: &str, to: &str) -> Transform {
    Transform::new(&system(from), &system(to)).unwrap_or_else(|fault| panic!("{fault}"))
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.6 Description 3d] geographic to
/// geocentric (EPSG 9602) on the Guidance Note example, 53°48′33.82″N
/// 2°07′46.38″E 73 m, to the millimetre, and back to 10⁻¹⁰ degree.
#[test]
fn geographic_to_geocentric_matches_the_worked_example() {
    let point = [dms(2.0_f64, 7.0_f64, 46.38_f64), dms(53.0_f64, 48.0_f64, 33.82_f64), 73.0_f64];
    let ecef = transform(WGS84, WGS84_ECEF).apply(point).unwrap();
    near(ecef, [3_771_793.968_f64, 140_253.342_f64, 5_124_304.349_f64], 0.0005_f64);
    let back = transform(WGS84_ECEF, WGS84).apply(ecef).unwrap();
    near([back[0], back[1]], [point[0], point[1]], 1e-10_f64);
    assert!((back[2] - 73.0_f64).abs() < 1e-6_f64);
    let direct = Ellipsoid::wgs84().geocentric(point[0].to_radians(), point[1].to_radians(), point[2]);
    near(direct, ecef, 1e-9_f64);
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.6 Description 3d] the position
/// vector Helmert shift (EPSG 9606) on the Guidance Note example, WGS 72
/// to WGS 84, and its inverse. The formula gives X = 3 657 660.774 from
/// the printed source, within a centimetre of the printed 3 657 660.78;
/// Y and Z agree to the millimetre.
#[test]
fn helmert_shift_matches_the_worked_example() {
    let source = [3_657_660.66_f64, 255_768.55_f64, 5_201_382.11_f64];
    let expected = [3_657_660.78_f64, 255_778.43_f64, 5_201_387.75_f64];
    near(transform(WGS72_ECEF, WGS84_ECEF).apply(source).unwrap(), expected, 0.01_f64);
    near(transform(WGS84_ECEF, WGS72_ECEF).apply(expected).unwrap(), source, 0.01_f64);
    let shift =
        Shift::new(&Helmert { translation: [0.0_f64, 0.0_f64, 4.5_f64], rotation: [0.0_f64, 0.0_f64, 0.554_f64], scale: 0.219_f64 });
    near(shift.inverse(shift.forward(source)), source, 1e-6_f64);
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.6 Description 3d] AXIS order
/// (latitude first) is honoured on input and output, the prime meridian
/// shifts longitudes, and angular units other than degrees convert.
#[test]
fn axes_meridians_and_units_apply() {
    let lat_lon = r#"GEOGCS["WGS 84",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433],AXIS["Lat",NORTH],AXIS["Lon",EAST]]"#;
    let swapped = transform(WGS84, lat_lon).apply_2d([10.0_f64, 50.0_f64]).unwrap();
    near(swapped, [50.0_f64, 10.0_f64], 1e-12_f64);
    let paris = r#"GEOGCS["NTF (Paris)",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563]],PRIMEM["Paris",2.5969213],UNIT["grad",0.015707963267949]]"#;
    let greenwich = transform(paris, WGS84).apply_2d([0.0_f64, 50.0_f64]).unwrap();
    near(greenwich, [2.337_229_17_f64, 45.0_f64], 1e-7_f64);
    let back = transform(WGS84, paris).apply_2d(greenwich).unwrap();
    near(back, [0.0_f64, 50.0_f64], 1e-9_f64);
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.6 Description 3c] F15 when a
/// datum has no shift to WGS 84, when a geocentric system is given no z,
/// and when a point is outside a projection's domain (a pole, on the
/// ellipsoid and on the sphere of Pseudo-Mercator); a failed batch
/// leaves the coordinates unchanged.
#[test]
fn failures_raise_failed_to_transform() {
    let unknown = r#"GEOGCS["Local",DATUM["Somewhere",SPHEROID["Bessel",6377397.155,299.1528128]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]]"#;
    let fault = Transform::new(&system(unknown), &system(WGS84)).expect_err("no shift");
    assert_eq!(fault.condition, Condition::FailedToTransform);
    let geocentric = transform(WGS84, WGS84_ECEF);
    assert_eq!(geocentric.apply_2d([0.0_f64, 0.0_f64]).map_err(|error| error.condition), Err(Condition::FailedToTransform));
    let mercator = r#"PROJCS["Mercator",GEOGCS["WGS 84",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]],PROJECTION["Mercator_1SP"],UNIT["metre",1]]"#;
    let forward = transform(WGS84, mercator);
    let mut batch = [10.0_f64, 10.0_f64, 7.0_f64, 0.0_f64, 90.0_f64, 7.0_f64];
    let before = batch;
    assert_eq!(forward.apply_all(&mut batch, 3, false).map_err(|error| error.condition), Err(Condition::FailedToTransform));
    assert_eq!(batch, before);
    let mut good = [10.0_f64, 10.0_f64, 7.0_f64];
    forward.apply_all(&mut good, 3, false).unwrap();
    assert!((good[2] - 7.0_f64).abs() < f64::EPSILON && good[0] > 1_000_000.0_f64);
    assert_eq!(forward.apply_all(&mut [1.0_f64], 1, false).map_err(|error| error.condition), Err(Condition::InvalidArgument));
    let pseudo = transform(WGS84, &super::seed(3857));
    assert_eq!(pseudo.apply_2d([0.0_f64, 90.0_f64]).map_err(|error| error.condition), Err(Condition::FailedToTransform));
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.6 Description 3d] a projected to
/// projected transform across datums goes through geographic coordinates
/// and the shifts, and round trips to a micrometre with heights. Without
/// them each datum's zero height is a different surface, and the round
/// trip moves the point by millimetres.
#[test]
fn projected_to_projected_round_trips() {
    let bng = r#"PROJCS["OSGB 1936 / British National Grid",GEOGCS["OSGB 1936",DATUM["OSGB_1936",SPHEROID["Airy 1830",6377563.396,299.3249646],TOWGS84[446.448,-125.157,542.06,0.15,0.247,0.842,-20.489]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]],PROJECTION["Transverse_Mercator"],PARAMETER["latitude_of_origin",49],PARAMETER["central_meridian",-2],PARAMETER["scale_factor",0.9996012717],PARAMETER["false_easting",400000],PARAMETER["false_northing",-100000],UNIT["metre",1]]"#;
    let utm = super::seed(32630);
    let there = transform(bng, &utm).apply([530_000.0_f64, 180_000.0_f64, 0.0_f64]).unwrap();
    let back = transform(&utm, bng).apply(there).unwrap();
    near(back, [530_000.0_f64, 180_000.0_f64, 0.0_f64], 1e-6_f64);
    let flat = transform(bng, &utm).apply_2d([530_000.0_f64, 180_000.0_f64]).unwrap();
    let flat_back = transform(&utm, bng).apply_2d(flat).unwrap();
    near(flat_back, [530_000.0_f64, 180_000.0_f64], 0.01_f64);
    assert!((there[0] - 699_000.0_f64).abs() < 2_000.0_f64, "{there:?}");
}
