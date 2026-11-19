//! Requirements at the edges of the reference system engine: every
//! subcondition's code, ellipsoids at their limits, malformed definitions,
//! axes the transform cannot take, projections of no valid parameters,
//! and geodesics short, nearly antipodal and on a flattened ellipsoid.

use super::super::ellipsoid::Ellipsoid;
use super::super::geodesic::Geodesic;
use super::super::number;
use super::super::system::SpatialRefSys;
use super::super::transform::Transform;
use super::super::{Condition, Fault, wkt};
use super::{near, system};

const WGS84: &str = r#"GEOGCS["WGS 84",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]]"#;
const METRE: &str = r#"UNIT["metre",1]"#;

/// WGS 84 with the axes `axes`, each a name and direction.
fn with_axes(axes: &[(&str, &str)]) -> String {
    let list = axes.iter().fold(String::new(), |out, (name, direction)| out + &format!(",AXIS[\"{name}\",{direction}]"));
    format!("{}{list}]", WGS84.strip_suffix(']').unwrap())
}

/// A projected system on WGS 84 of `projection` and `parameters`.
fn projcs(projection: &str, parameters: &[(&str, f64)]) -> String {
    let list = parameters.iter().fold(String::new(), |out, (name, value)| out + &format!(",PARAMETER[\"{name}\",{value}]"));
    format!("PROJCS[\"test\",{WGS84},PROJECTION[\"{projection}\"]{list},{METRE}]")
}

/// The condition of the fault reading `text`.
fn read_fault(text: &str) -> Condition {
    wkt::read(text).expect_err("a fault").condition
}

/// The condition of the fault of the transform from WGS 84 to `text`.
fn transform_fault(text: &str) -> Condition {
    Transform::new(&system(WGS84), &system(text)).expect_err("a fault").condition
}

/// Verifies: [ISO/IEC 13249-3:2016 §4.2, Table 16] each subcondition has
/// its subclass code and name, and a fault shows them both.
#[test]
fn every_subcondition_has_its_code() {
    let all = [
        (Condition::InvalidArgument, "F02", "invalid argument"),
        (Condition::DivisionByZero, "F13", "attempted division by zero"),
        (Condition::UnsupportedUnit, "F14", "unsupported unit specified"),
        (Condition::FailedToTransform, "F15", "failed to transform geometry"),
        (Condition::EmptyPoint, "F17", "empty point value"),
        (Condition::PointNotWellFormed, "F18", "point value not well formed"),
        (Condition::PointsEqual, "F19", "points are equal"),
        (Condition::NotALine, "F20", "linestring is not a line"),
        (Condition::DegenerateLine, "F21", "degenerate line has no direction"),
        (Condition::InvalidText, "F22", "invalid well-known text representation"),
        (Condition::InvalidGml, "F24", "invalid GML representation"),
        (Condition::UnknownSystem, "F59", "unknown spatial reference system"),
    ];
    for (condition, code, name) in all {
        assert_eq!((condition.subclass(), condition.text()), (code, name));
        assert_eq!(Fault::new(condition, "m").to_string(), format!("2F{code}: m"));
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 §14.1.9 Description 1k] an ellipsoid
/// needs a positive axis and an inverse flattening of 0 or above 1; its
/// latitude conversions keep infinite values and the pole, and the point
/// on its axis is at a pole.
#[test]
fn ellipsoids_at_their_limits() {
    assert_eq!(Ellipsoid::new(0.0_f64, 298.0_f64).expect_err("a fault").condition, Condition::InvalidArgument);
    assert_eq!(Ellipsoid::new(6_378_137.0_f64, 0.5_f64).expect_err("a fault").condition, Condition::InvalidArgument);
    let e = Ellipsoid::wgs84();
    assert_eq!(e.conformal_tan(f64::INFINITY), f64::INFINITY);
    assert_eq!(e.geodetic_tan(f64::INFINITY), f64::INFINITY);
    near([e.from_q(e.q(std::f64::consts::FRAC_PI_2))], [std::f64::consts::FRAC_PI_2], 1e-9_f64);
    let pole = e.geodetic([0.0_f64, 0.0_f64, 6_356_752.314_245_f64]);
    near(pole, [0.0_f64, std::f64::consts::FRAC_PI_2, 0.0_f64], 1e-3_f64);
}

/// Verifies: [ISO/IEC 13249-3:2016 §14.1.9] definitions nested too deeply,
/// lacking a comma, with an AXIS of no direction word, a number for a
/// name or a name for a number are invalid text (F22); an authority code
/// may be a number, and an EPSG code names the system's SRID.
#[test]
fn malformed_definitions() {
    let deep = format!("GEOGCS[\"x\",{}1{}]", "A[".repeat(8), "]".repeat(8));
    assert_eq!(read_fault(&deep), Condition::InvalidText);
    assert_eq!(read_fault(r#"GEOGCS["WGS 84" DATUM["D",SPHEROID["S",6378137,298]]]"#), Condition::InvalidText);
    assert_eq!(read_fault(&with_axes(&[("Lat", "\"NORTH\"")])), Condition::InvalidText);
    assert_eq!(
        read_fault(r#"GEOGCS["WGS 84",DATUM[5,SPHEROID["S",6378137,298]],PRIMEM["G",0],UNIT["degree",0.0174532925199433]]"#),
        Condition::InvalidText
    );
    assert_eq!(
        read_fault(r#"GEOGCS["WGS 84",DATUM["D",SPHEROID["S","a",298]],PRIMEM["G",0],UNIT["degree",0.0174532925199433]]"#),
        Condition::InvalidText
    );
    let coded = format!("{},AUTHORITY[\"EPSG\",4326]]", WGS84.strip_suffix(']').unwrap());
    let srs = SpatialRefSys::from_text(&coded).unwrap();
    assert_eq!(srs.srid(), Some(4326_i32));
    let geocentric =
        system(r#"GEOCCS["ECEF",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563]],PRIMEM["Greenwich",0],UNIT["metre",1]]"#);
    assert_eq!(geocentric.linear_unit().map(|u| u.factor), Some(1.0_f64));
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.6 Description 3d] axes facing
/// west and south turn the signs, a vertical third axis is kept; axes that
/// are not east and north, a horizontal third axis and a geocentric
/// system's other axes fail to transform (F15), as does a point without
/// its z value.
#[test]
fn axes_the_transform_takes() {
    let turned = Transform::new(&system(WGS84), &system(&with_axes(&[("Lon", "WEST"), ("Lat", "SOUTH")]))).unwrap();
    near(turned.apply_2d([10.0_f64, 20.0_f64]).unwrap(), [-10.0_f64, -20.0_f64], 1e-12_f64);
    let raised = Transform::new(&system(WGS84), &system(&with_axes(&[("Lon", "EAST"), ("Lat", "NORTH"), ("h", "DOWN")]))).unwrap();
    near(raised.apply([10.0_f64, 20.0_f64, 5.0_f64]).unwrap(), [10.0_f64, 20.0_f64, -5.0_f64], 1e-9_f64);
    assert_eq!(transform_fault(&with_axes(&[("a", "UP"), ("b", "UP")])), Condition::FailedToTransform);
    assert_eq!(transform_fault(&with_axes(&[("Lon", "EAST"), ("Lat", "NORTH"), ("x", "EAST")])), Condition::FailedToTransform);
    let ecef = r#"GEOCCS["ECEF",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563]],PRIMEM["Greenwich",0],UNIT["metre",1],AXIS["X",EAST],AXIS["Y",NORTH],AXIS["Z",UP]]"#;
    assert_eq!(transform_fault(ecef), Condition::FailedToTransform);
    let same = Transform::new(&system(WGS84), &system(WGS84)).unwrap();
    assert_eq!(same.apply_all(&mut [1.0_f64, 2.0_f64], 2, true).expect_err("a fault").condition, Condition::InvalidArgument);
}

/// Verifies: [ISO/IEC 13249-3:2016 §14.1.9 Description 1i] a parameter no
/// method takes, a Mercator latitude of origin not zero, standard
/// parallels of no cone, and polar stereographic origins off the pole fail
/// to transform (F15); the inverse of the false origin is the origin.
#[test]
fn projections_of_no_valid_parameters() {
    assert_eq!(transform_fault(&projcs("Mercator_1SP", &[("bogus", 1.0_f64)])), Condition::FailedToTransform);
    assert_eq!(transform_fault(&projcs("Mercator_1SP", &[("latitude_of_origin", 10.0_f64)])), Condition::FailedToTransform);
    let opposite = [("standard_parallel_1", 10.0_f64), ("standard_parallel_2", -10.0_f64)];
    assert_eq!(transform_fault(&projcs("Lambert_Conformal_Conic_2SP", &opposite)), Condition::FailedToTransform);
    assert_eq!(transform_fault(&projcs("Albers_Conic_Equal_Area", &opposite)), Condition::FailedToTransform);
    assert_eq!(
        transform_fault(&projcs("Polar_Stereographic_variant_A", &[("latitude_of_origin", 45.0_f64)])),
        Condition::FailedToTransform
    );
    assert_eq!(transform_fault(&projcs("Polar_Stereographic_variant_B", &[("standard_parallel", 0.0_f64)])), Condition::FailedToTransform);
    let origin =
        [("latitude_of_origin", 52.0_f64), ("central_meridian", 5.0_f64), ("false_easting", 1000.0_f64), ("false_northing", 2000.0_f64)];
    for method in ["Oblique_Stereographic", "Lambert_Azimuthal_Equal_Area"] {
        let projected = system(&projcs(method, &origin));
        let back = Transform::new(&projected, &system(WGS84)).unwrap();
        near(back.apply_2d([1000.0_f64, 2000.0_f64]).unwrap(), [5.0_f64, 52.0_f64], 1e-9_f64);
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 §14.1.9] a number's exponent needs
/// digits, and a value that rounds to zero is written without a sign.
#[test]
fn numbers_at_their_limits() {
    assert_eq!(number::value("1e+"), None);
    assert_eq!(number::value("1e5"), Some(100_000.0_f64));
    assert_eq!(number::fixed(-0.0001_f64, 2), "0.00");
}

/// The direct problem from the first point along the inverse's azimuth
/// and distance comes back to the second point.
fn round_trip(g: &Geodesic, first: [f64; 2], second: [f64; 2]) {
    let solved = g.inverse(first[0], first[1], second[0], second[1]);
    let [lon, lat, _] = g.direct(first[0], first[1], solved.azimuth1, solved.distance);
    let east = (lon - second[0]).to_radians();
    assert!(east.sin().abs() < 1e-9_f64 && east.cos() > 0.0_f64, "{lon} is not {} ({solved:?})", second[0]);
    assert!((lat - second[1]).abs() < 1e-9_f64, "{lat} is not {} ({solved:?})", second[1]);
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.41 Description 2a iv] short,
/// nearly antipodal and equatorial geodesics, and those of an ellipsoid
/// flattened beyond the series' first order, are the shortest paths the
/// direct problem retraces.
#[test]
fn geodesics_short_antipodal_and_flattened() {
    let g = Geodesic::wgs84();
    for (first, second) in [
        ([10.0_f64, 40.0_f64], [10.0001_f64, 40.0001_f64]),
        ([0.0_f64, 0.0_f64], [0.0001_f64, 0.0_f64]),
        ([0.0_f64, 0.5_f64], [179.5_f64, -0.5_f64]),
        ([0.0_f64, 0.0_f64], [179.7_f64, 0.0_f64]),
        ([0.0_f64, -30.0_f64], [179.9_f64, 29.9_f64]),
        ([0.0_f64, 0.0_f64], [179.8_f64, 0.01_f64]),
    ] {
        round_trip(&g, first, second);
    }
    let flat = Geodesic::new(&Ellipsoid::new(6_378_137.0_f64, 50.0_f64).unwrap());
    round_trip(&flat, [0.0_f64, 10.0_f64], [40.0_f64, 50.0_f64]);
    let polar = flat.direct(0.0_f64, 0.0_f64, 0.0_f64, 5_000_000.0_f64);
    assert!(polar[1] > 0.0_f64 && polar[1] < 90.0_f64, "{polar:?}");
}

/// Verifies: [ISO/IEC 13249-3:2016 §8.1.2 Description 2a iv] a geodesic
/// ring enclosing more than half the ellipsoid has the area of its
/// complement, of the sign its direction gives, and the reversed ring the
/// opposite area.
#[test]
fn rings_beyond_half_the_ellipsoid() {
    let g = Geodesic::wgs84();
    let ring = [[0.0_f64, -10.0_f64], [90.0_f64, -10.0_f64], [180.0_f64, -10.0_f64], [-90.0_f64, -10.0_f64]];
    let (_, forward) = g.polygon(&ring);
    let reversed: Vec<[f64; 2]> = ring.iter().rev().copied().collect();
    let (_, backward) = g.polygon(&reversed);
    let half = g.ellipsoid_area() / 2.0_f64;
    assert!(forward.abs() <= half && backward.abs() <= half, "{forward} {backward}");
    assert!((forward + backward).abs() < 1.0_f64, "{forward} {backward}");
}

/// Verifies: [ISO/IEC 13249-3:2016 §14.1.9, §3.1.4 d] each kind of system
/// has its name, and only a geographic one is geodetic; a definition of
/// text the grammar does not know is invalid (F22), and a two-parallel
/// conic projection without its parallel fails to transform (F15).
#[test]
fn systems_by_kind() {
    let ecef = r#"GEOCCS["ECEF",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563]],PRIMEM["Greenwich",0],UNIT["metre",1]]"#;
    let kinds = [(WGS84.to_owned(), "WGS 84", true), (ecef.to_owned(), "ECEF", false), (projcs("Mercator_1SP", &[]), "test", false)];
    for (text, name, geodetic) in kinds {
        let cs = system(&text);
        assert_eq!((cs.name(), cs.is_geodetic()), (name, geodetic));
    }
    assert_eq!(read_fault(r#"GEOGCS["WGS 84",@]"#), Condition::InvalidText);
    assert_eq!(transform_fault(&projcs("Lambert_Conformal_Conic_2SP", &[])), Condition::FailedToTransform);
}
