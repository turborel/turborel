//! Requirements of the geodesic distance: the inverse and direct problems
//! against published solutions (Karney 2013 and the GeographicLib
//! documentation, Vincenty 1975), and the distance from a point to a
//! geodesic segment.

use super::super::ellipsoid::Ellipsoid;
use super::super::geodesic::Geodesic;
use super::dms;

/// Bessel 1841, the ellipsoid of Vincenty's test lines.
fn bessel() -> Geodesic {
    Geodesic::new(&Ellipsoid::new(6_377_397.155_f64, 299.152_812_8_f64).unwrap())
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.41 Description 2a iv] the
/// geodetic distance between points is the shortest geodesic: Wellington
/// to Salamanca and JFK to Heathrow match the GeographicLib solutions to
/// the millimetre and their azimuths to 10⁻⁸ degree.
#[test]
fn inverse_matches_the_published_solutions() {
    let g = Geodesic::wgs84();
    let w = g.inverse(174.81_f64, -41.32_f64, -5.50_f64, 40.96_f64);
    assert!((w.distance - 19_959_679.267_f64).abs() < 0.001_f64, "{w:?}");
    assert!((w.azimuth1 - 161.067_669_986_f64).abs() < 1e-8_f64, "{w:?}");
    assert!((w.azimuth2 - 18.825_195_123_f64).abs() < 1e-8_f64, "{w:?}");
    let j = g.inverse(-73.8_f64, 40.6_f64, -0.5_f64, 51.6_f64);
    assert!((j.distance - 5_551_759.400_f64).abs() < 0.001_f64, "{j:?}");
    assert!((j.azimuth1 - 51.198_882_845_f64).abs() < 1e-8_f64, "{j:?}");
    assert!((j.azimuth2 - 107.821_776_735_f64).abs() < 1e-8_f64, "{j:?}");
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.41 Description 2a iv] the
/// special cases: a meridian, the equator, equatorial antipodes (whose
/// shortest geodesic runs over a pole) and coincident points.
#[test]
fn inverse_handles_meridians_equator_and_antipodes() {
    let g = Geodesic::wgs84();
    let quarter = g.inverse(0.0_f64, 0.0_f64, 0.0_f64, 90.0_f64);
    assert!((quarter.distance - 10_001_965.729_f64).abs() < 0.001_f64, "{quarter:?}");
    assert!(quarter.azimuth1.abs() < 1e-12_f64, "{quarter:?}");
    let degree = g.inverse(0.0_f64, 0.0_f64, 1.0_f64, 0.0_f64);
    assert!((degree.distance - 111_319.490_793_f64).abs() < 1e-6_f64, "{degree:?}");
    assert!((degree.azimuth1 - 90.0_f64).abs() < 1e-12_f64, "{degree:?}");
    let antipodes = g.inverse(0.0_f64, 0.0_f64, 180.0_f64, 0.0_f64);
    assert!((antipodes.distance - 2.0_f64 * 10_001_965.729_f64).abs() < 0.002_f64, "{antipodes:?}");
    let near = g.inverse(0.0_f64, 0.0_f64, 179.5_f64, 0.5_f64);
    let back = g.direct(0.0_f64, 0.0_f64, near.azimuth1, near.distance);
    assert!((back[0] - 179.5_f64).abs() < 1e-9_f64 && (back[1] - 0.5_f64).abs() < 1e-9_f64, "{near:?} {back:?}");
    let same = g.inverse(10.0_f64, 20.0_f64, 10.0_f64, 20.0_f64);
    assert!(same.distance == 0.0_f64, "{same:?}");
    assert!(g.inverse(0.0_f64, 91.0_f64, 0.0_f64, 0.0_f64).distance.is_nan());
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.41 Description 2a iv] Vincenty's
/// test line (b) on the Bessel ellipsoid, 55°45′N to 33°26′S across
/// 108°13′ of longitude, 14 110 526.170 m.
#[test]
fn inverse_matches_vincenty_on_bessel() {
    let g = bessel();
    let line = g.inverse(0.0_f64, dms(55.0_f64, 45.0_f64, 0.0_f64), dms(108.0_f64, 13.0_f64, 0.0_f64), -dms(33.0_f64, 26.0_f64, 0.0_f64));
    assert!((line.distance - 14_110_526.170_f64).abs() < 0.001_f64, "{line:?}");
    assert!((line.azimuth1 - dms(96.0_f64, 36.0_f64, 8.799_f64)).abs() < 1e-6_f64, "{line:?}");
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.41 Description 2a iv] the
/// direct problem inverts the inverse problem to a micrometre over a
/// spread of lines, including ones that cross the antimeridian.
#[test]
fn direct_inverts_the_inverse() {
    let g = Geodesic::wgs84();
    let lines = [
        [-73.8_f64, 40.6_f64, -0.5_f64, 51.6_f64],
        [174.81_f64, -41.32_f64, -5.50_f64, 40.96_f64],
        [170.0_f64, 10.0_f64, -170.0_f64, -10.0_f64],
        [0.0_f64, -89.0_f64, 90.0_f64, 89.0_f64],
    ];
    for [lon1, lat1, lon2, lat2] in lines {
        let inverse = g.inverse(lon1, lat1, lon2, lat2);
        let [lon, lat, azimuth] = g.direct(lon1, lat1, inverse.azimuth1, inverse.distance);
        assert!((lon - lon2).abs() < 1e-11_f64 && (lat - lat2).abs() < 1e-11_f64, "{inverse:?} gives {lon} {lat}");
        assert!((azimuth - inverse.azimuth2).abs() < 1e-9_f64, "{inverse:?} gives {azimuth}");
    }
    let [lon, lat, _] = g.direct(0.0_f64, 0.0_f64, 0.0_f64, 10_001_965.729_f64);
    assert!((lat - 90.0_f64).abs() < 1e-8_f64 && lon.abs() < 1e-8_f64, "{lon} {lat}");
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.41 Description 2a iv] the
/// geodetic ST_Length of a path is the sum of its geodesic segments.
#[test]
fn length_sums_the_segments() {
    let g = Geodesic::wgs84();
    let path = [[0.0_f64, 0.0_f64], [1.0_f64, 0.0_f64], [1.0_f64, 0.0_f64], [2.0_f64, 0.0_f64]];
    assert!((g.length(&path) - 2.0_f64 * 111_319.490_793_f64).abs() < 1e-6_f64);
    assert_eq!(g.length(&path[..1]), 0.0_f64);
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.41 Description 2a iv] the least
/// distance from a point to a geodesic segment: interior feet (MC/DC:
/// both end slopes point inward), each endpoint nearest (the slope at A
/// points away, then the slope at B), and a degenerate segment.
#[test]
fn point_segment_distance_finds_the_foot() {
    let g = Geodesic::wgs84();
    let along = [[-1.0_f64, 0.0_f64], [1.0_f64, 0.0_f64]];
    let north = g.inverse(0.0_f64, 0.0_f64, 0.0_f64, 1.0_f64).distance;
    assert!((g.point_segment([0.0_f64, 1.0_f64], along[0], along[1]) - north).abs() < 1e-6_f64);
    let beyond_a = g.inverse(-2.0_f64, 1.0_f64, -1.0_f64, 0.0_f64).distance;
    assert!((g.point_segment([-2.0_f64, 1.0_f64], along[0], along[1]) - beyond_a).abs() < 1e-6_f64);
    let beyond_b = g.inverse(3.0_f64, -1.0_f64, 1.0_f64, 0.0_f64).distance;
    assert!((g.point_segment([3.0_f64, -1.0_f64], along[0], along[1]) - beyond_b).abs() < 1e-6_f64);
    let point = g.point_segment([0.0_f64, 1.0_f64], [0.0_f64, 0.0_f64], [0.0_f64, 0.0_f64]);
    assert!((point - north).abs() < 1e-6_f64);
    let slanted = g.point_segment([10.0_f64, 50.0_f64], [0.0_f64, 40.0_f64], [20.0_f64, 45.0_f64]);
    let samples = (0_i32..=1000_i32).map(|i| f64::from(i) / 1000.0_f64);
    let segment = g.inverse(0.0_f64, 40.0_f64, 20.0_f64, 45.0_f64);
    let least = samples
        .map(|t| g.direct(0.0_f64, 40.0_f64, segment.azimuth1, t * segment.distance))
        .map(|[lon, lat, _]| g.inverse(10.0_f64, 50.0_f64, lon, lat).distance)
        .fold(f64::INFINITY, f64::min);
    assert!(slanted <= least && least - slanted < 1.0_f64, "{slanted} {least}");
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.41 Description 2a iv] the least
/// distance between geodesic segments: zero when they cross, else from
/// the nearest endpoint.
#[test]
fn segment_distance_is_zero_when_crossing() {
    let g = Geodesic::wgs84();
    let crossing = g.segment_segment([[-1.0_f64, -1.0_f64], [1.0_f64, 1.0_f64]], [[-1.0_f64, 1.0_f64], [1.0_f64, -1.0_f64]]);
    assert_eq!(crossing, 0.0_f64);
    let apart = g.segment_segment([[0.0_f64, 0.0_f64], [1.0_f64, 0.0_f64]], [[0.0_f64, 1.0_f64], [1.0_f64, 1.0_f64]]);
    let expected = g.inverse(0.0_f64, 0.0_f64, 0.0_f64, 1.0_f64).distance;
    assert!((apart - expected).abs() < 1.0_f64, "{apart} {expected}");
}
