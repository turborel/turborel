//! Requirements of the map projections: each method forward and inverse
//! against its published worked example (EPSG Guidance Note 7-2, 2019, and
//! Snyder 1987), and round trips to a tenth of a millimetre.

use super::super::projection::wrap;
use super::{dms, projects};

const AIRY: &str = r#"GEOGCS["OSGB 1936",DATUM["OSGB_1936",SPHEROID["Airy 1830",6377563.396,299.3249646]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]]"#;
const CLARKE_1866: &str = r#"GEOGCS["NAD27",DATUM["North_American_Datum_1927",SPHEROID["Clarke 1866",6378206.4,294.9786982]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]]"#;
const BESSEL: &str = r#"GEOGCS["Bessel",DATUM["Bessel_1841",SPHEROID["Bessel 1841",6377397.155,299.1528128]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]]"#;
const WGS84: &str = r#"GEOGCS["WGS 84",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]]"#;

fn projcs(geogcs: &str, projection: &str, parameters: &[(&str, f64)], unit: &str) -> String {
    let parameters = parameters.iter().fold(String::new(), |out, (name, value)| out + &format!(",PARAMETER[\"{name}\",{value}]"));
    format!("PROJCS[\"test\",{geogcs},PROJECTION[\"{projection}\"]{parameters},{unit}]")
}

const METRE: &str = r#"UNIT["metre",1]"#;

/// Verifies: [ISO/IEC 13249-3:2016 §14.1.9 Description 1d] Transverse
/// Mercator (EPSG 9807) by the Krüger series, on the British National
/// Grid example of Guidance Note 7-2 as computed by its JHS formulas
/// (latitude of origin 49°N, so the meridian arc offset is exercised) and
/// on Snyder's UTM example.
#[test]
fn transverse_mercator_matches_the_worked_examples() {
    let bng = [
        ("latitude_of_origin", 49.0_f64),
        ("central_meridian", -2.0_f64),
        ("scale_factor", 0.999_601_271_7_f64),
        ("false_easting", 400_000.0_f64),
        ("false_northing", -100_000.0_f64),
    ];
    projects(&projcs(AIRY, "Transverse_Mercator", &bng, METRE), [0.5_f64, 50.5_f64], [577_274.98_f64, 69_740.49_f64], 0.006_f64);
    let utm = [("central_meridian", -75.0_f64), ("scale_factor", 0.9996_f64), ("false_easting", 500_000.0_f64)];
    projects(&projcs(CLARKE_1866, "Transverse Mercator", &utm, METRE), [-73.5_f64, 40.5_f64], [627_106.5_f64, 4_484_124.4_f64], 0.06_f64);
}

/// Verifies: [ISO/IEC 13249-3:2016 §14.1.9 Description 1d] Mercator
/// variant A (9804, scale factor) and variant B (9805, standard parallel),
/// and Popular Visualisation Pseudo Mercator (1024), on the Guidance Note
/// examples.
#[test]
fn mercator_variants_match_the_worked_examples() {
    let a = [
        ("central_meridian", 110.0_f64),
        ("scale_factor", 0.997_f64),
        ("false_easting", 3_900_000.0_f64),
        ("false_northing", 900_000.0_f64),
    ];
    projects(&projcs(BESSEL, "Mercator_1SP", &a, METRE), [120.0_f64, -3.0_f64], [5_009_726.58_f64, 569_150.82_f64], 0.006_f64);
    let krassowsky = r#"GEOGCS["Pulkovo 1942",DATUM["Pulkovo_1942",SPHEROID["Krassowsky 1940",6378245,298.3]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]]"#;
    let b = [("standard_parallel_1", 42.0_f64), ("central_meridian", 51.0_f64)];
    projects(&projcs(krassowsky, "Mercator_2SP", &b, METRE), [53.0_f64, 53.0_f64], [165_704.29_f64, 5_171_848.07_f64], 0.006_f64);
    let point = [-dms(100.0_f64, 20.0_f64, 0.0_f64), dms(24.0_f64, 22.0_f64, 54.433_f64)];
    projects(&projcs(WGS84, "Popular Visualisation Pseudo Mercator", &[], METRE), point, [-11_169_055.58_f64, 2_800_000.00_f64], 0.006_f64);
}

/// Verifies: [ISO/IEC 13249-3:2016 §14.1.9 Description 1d] Lambert Conic
/// Conformal with one standard parallel (9801, Jamaica) and two (9802,
/// Texas South Central in US survey feet, so the linear unit scales the
/// false easting and the result).
#[test]
fn lambert_conic_conformal_matches_the_worked_examples() {
    let one = [
        ("latitude_of_origin", 18.0_f64),
        ("central_meridian", -77.0_f64),
        ("scale_factor", 1.0_f64),
        ("false_easting", 250_000.0_f64),
        ("false_northing", 150_000.0_f64),
    ];
    let point = [-dms(76.0_f64, 56.0_f64, 37.26_f64), dms(17.0_f64, 55.0_f64, 55.80_f64)];
    projects(&projcs(CLARKE_1866, "Lambert_Conformal_Conic_1SP", &one, METRE), point, [255_966.58_f64, 142_493.51_f64], 0.006_f64);
    let two = [
        ("standard_parallel_1", dms(28.0_f64, 23.0_f64, 0.0_f64)),
        ("standard_parallel_2", dms(30.0_f64, 17.0_f64, 0.0_f64)),
        ("latitude_of_origin", dms(27.0_f64, 50.0_f64, 0.0_f64)),
        ("central_meridian", -99.0_f64),
        ("false_easting", 2_000_000.0_f64),
        ("false_northing", 0.0_f64),
    ];
    let feet = r#"UNIT["US survey foot",0.304800609601219]"#;
    projects(
        &projcs(CLARKE_1866, "Lambert_Conformal_Conic_2SP", &two, feet),
        [-96.0_f64, 28.5_f64],
        [2_963_503.91_f64, 254_759.80_f64],
        0.006_f64,
    );
}

/// Verifies: [ISO/IEC 13249-3:2016 §14.1.9 Description 1d] Albers Equal
/// Area (9822) on Snyder's ellipsoidal example (p. 292), whose inverse
/// iterates on q.
#[test]
fn albers_matches_the_worked_example() {
    let params = [
        ("standard_parallel_1", 29.5_f64),
        ("standard_parallel_2", 45.5_f64),
        ("latitude_of_center", 23.0_f64),
        ("longitude_of_center", -96.0_f64),
    ];
    projects(
        &projcs(CLARKE_1866, "Albers_Conic_Equal_Area", &params, METRE),
        [-75.0_f64, 35.0_f64],
        [1_885_472.7_f64, 1_535_925.0_f64],
        0.06_f64,
    );
}

/// Verifies: [ISO/IEC 13249-3:2016 §14.1.9 Description 1d] Polar
/// Stereographic variants A (9810, UPS North), B (9829, Australian
/// Antarctic, a south pole) and C (9830, Terre Adélie, false origin on
/// the standard parallel).
#[test]
fn polar_stereographic_variants_match_the_worked_examples() {
    let a = [
        ("latitude_of_origin", 90.0_f64),
        ("central_meridian", 0.0_f64),
        ("scale_factor", 0.994_f64),
        ("false_easting", 2_000_000.0_f64),
        ("false_northing", 2_000_000.0_f64),
    ];
    projects(&projcs(WGS84, "Polar_Stereographic", &a, METRE), [44.0_f64, 73.0_f64], [3_320_416.75_f64, 632_668.43_f64], 0.006_f64);
    let b = [
        ("standard_parallel_1", -71.0_f64),
        ("central_meridian", 70.0_f64),
        ("false_easting", 6_000_000.0_f64),
        ("false_northing", 6_000_000.0_f64),
    ];
    projects(
        &projcs(WGS84, "Polar Stereographic (variant B)", &b, METRE),
        [120.0_f64, -75.0_f64],
        [7_255_380.79_f64, 7_053_389.56_f64],
        0.006_f64,
    );
    let hayford = r#"GEOGCS["Petrels 1972",DATUM["Petrels_1972",SPHEROID["International 1924",6378388,297]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]]"#;
    let c = [
        ("standard_parallel_1", -67.0_f64),
        ("central_meridian", 140.0_f64),
        ("false_easting", 300_000.0_f64),
        ("false_northing", 200_000.0_f64),
    ];
    let point = [dms(140.0_f64, 4.0_f64, 17.040_f64), -dms(66.0_f64, 36.0_f64, 18.820_f64)];
    projects(&projcs(hayford, "Polar_Stereographic_variant_C", &c, METRE), point, [303_169.52_f64, 244_055.72_f64], 0.006_f64);
}

/// Verifies: [ISO/IEC 13249-3:2016 §14.1.9 Description 1d] Oblique
/// Stereographic (9809) on the Amersfoort / RD New example, printed to the
/// millimetre.
#[test]
fn oblique_stereographic_matches_the_worked_example() {
    let params = [
        ("latitude_of_origin", dms(52.0_f64, 9.0_f64, 22.178_f64)),
        ("central_meridian", dms(5.0_f64, 23.0_f64, 15.5_f64)),
        ("scale_factor", 0.999_907_9_f64),
        ("false_easting", 155_000.0_f64),
        ("false_northing", 463_000.0_f64),
    ];
    projects(&projcs(BESSEL, "Oblique_Stereographic", &params, METRE), [6.0_f64, 53.0_f64], [196_105.283_f64, 557_057.739_f64], 0.0006_f64);
}

/// Verifies: [ISO/IEC 13249-3:2016 §14.1.9 Description 1d] Lambert
/// Azimuthal Equal Area (9820), oblique aspect on ETRS89-LAEA, and the
/// polar aspect's round trip at both poles.
#[test]
fn lambert_azimuthal_equal_area_matches_the_worked_example() {
    let grs80 = r#"GEOGCS["ETRS89",DATUM["European_Terrestrial_Reference_System_1989",SPHEROID["GRS 1980",6378137,298.257222101]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]]"#;
    let params = [
        ("latitude_of_center", 52.0_f64),
        ("longitude_of_center", 10.0_f64),
        ("false_easting", 4_321_000.0_f64),
        ("false_northing", 3_210_000.0_f64),
    ];
    projects(
        &projcs(grs80, "Lambert_Azimuthal_Equal_Area", &params, METRE),
        [5.0_f64, 50.0_f64],
        [3_962_799.45_f64, 2_999_718.85_f64],
        0.006_f64,
    );
    for pole in [90.0_f64, -90.0_f64] {
        let polar = projcs(grs80, "Lambert_Azimuthal_Equal_Area", &[("latitude_of_center", pole)], METRE);
        let point = [30.0_f64, pole * 0.75_f64];
        let projected = super::system(&polar);
        let geographic = super::base(&projected);
        let there = super::Transform::new(&geographic, &projected).unwrap().apply_2d(point).unwrap();
        super::near(super::Transform::new(&projected, &geographic).unwrap().apply_2d(there).unwrap(), point, 1e-9_f64);
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 §14.1.9 Description 1d] Equidistant
/// Cylindrical (1028, World Equidistant Cylindrical), whose northing is
/// the meridian arc, and the spherical form (1029).
#[test]
fn equidistant_cylindrical_matches_the_worked_example() {
    let params = [("standard_parallel_1", 0.0_f64), ("central_meridian", 0.0_f64)];
    projects(
        &projcs(WGS84, "Equidistant_Cylindrical", &params, METRE),
        [10.0_f64, 55.0_f64],
        [1_113_194.91_f64, 6_097_230.31_f64],
        0.006_f64,
    );
    let sphere = 6_378_137.0_f64 * 55.0_f64.to_radians();
    projects(
        &projcs(WGS84, "Equidistant Cylindrical (Spherical)", &params, METRE),
        [10.0_f64, 55.0_f64],
        [1_113_194.91_f64, sphere],
        0.006_f64,
    );
}

/// Verifies: [DESIGN] Longitude differences are reduced to [−π, π), so a
/// point across the antimeridian from the central meridian projects on
/// the near side.
#[test]
fn longitude_differences_wrap() {
    let pi = std::f64::consts::PI;
    assert!((wrap(3.0_f64 * pi / 2.0_f64) + pi / 2.0_f64).abs() < 1e-15_f64);
    assert!((wrap(-pi) + pi).abs() < 1e-15_f64);
    assert!((wrap(pi) + pi).abs() < 1e-15_f64);
    assert!(wrap(0.25_f64).abs() - 0.25_f64 < 1e-15_f64);
}
