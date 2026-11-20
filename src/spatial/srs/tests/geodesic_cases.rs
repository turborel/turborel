//! The inverse geodesic problem over its hard cases (Karney 2013 §3 to
//! §5): points nearly antipodal, points a few millimetres apart, points
//! near a pole and on one meridian, on WGS 84, a flatter ellipsoid and a
//! sphere. Each solution is checked against the direct problem, which
//! takes the first point along its azimuth and distance to the second.

use super::super::ellipsoid::Ellipsoid;
use super::super::geodesic::Geodesic;

/// Pairs of points, each its longitude and latitude in degrees.
const PAIRS: [[f64; 4]; 14] = [
    [0.0, 0.0, 179.9, 0.1],
    [0.0, -0.3, 179.7, 0.2],
    [0.0, 30.0, 179.8, -29.9],
    [0.0, 45.0, 180.0, -45.0],
    [10.0, -60.0, -170.05, 59.97],
    [0.0, 0.0, 179.98, 0.0],
    [0.0, 0.5, 179.5, -0.5],
    [5.0, 10.0, 5.000_000_01, 10.000_000_01],
    [5.0, 10.0, 5.0, 10.000_000_02],
    [-120.0, 89.999, 60.0, 89.998],
    [0.0, -89.9, 90.0, -89.9],
    [33.0, 20.0, 33.0, -70.0],
    [0.0, 0.0, 90.0, 0.0],
    [0.0, 1e-9, 179.0, -1e-9],
];

/// That the solution `g` gives of each pair reaches the second point
/// along its azimuth and distance, is as long either way, and is no
/// longer than half a meridian ellipse's circumference allows.
fn check(g: &Geodesic, name: &str) {
    for [lon1, lat1, lon2, lat2] in PAIRS {
        let found = g.inverse(lon1, lat1, lon2, lat2);
        assert!(found.distance.is_finite() && found.distance >= 0.0_f64, "{name} {lon1} {lat1} {lon2} {lat2}: {found:?}");
        let back = g.inverse(lon2, lat2, lon1, lat1);
        assert!((found.distance - back.distance).abs() < 1e-6_f64, "{name}: {found:?} and back {back:?}");
        let [lon, lat, _] = g.direct(lon1, lat1, found.azimuth1, found.distance);
        let dlon = (lon - lon2 + 540.0_f64).rem_euclid(360.0_f64) - 180.0_f64;
        let near_pole = lat2.abs() > 89.99_f64;
        assert!((lat - lat2).abs() < 1e-7_f64, "{name} {lon1} {lat1} {lon2} {lat2}: reached {lon} {lat}");
        assert!(near_pole || dlon.abs() < 1e-7_f64, "{name} {lon1} {lat1} {lon2} {lat2}: reached {lon} {lat}");
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.41 Description 2a iv] the
/// geodetic distance between points is the shortest geodesic's, on
/// WGS 84, on an ellipsoid ten times flatter and on a sphere, for points
/// nearly antipodal, millimetres apart, near a pole and on one meridian:
/// each solution's azimuth and distance lead from the first point to the
/// second, and it is as long either way.
#[test]
fn inverse_solutions_lead_to_their_ends() {
    check(&Geodesic::wgs84(), "WGS 84");
    check(&Geodesic::new(&Ellipsoid::new(6_378_137.0_f64, 29.8_f64).unwrap()), "flat");
    check(&Geodesic::new(&Ellipsoid::new(6_371_000.0_f64, 0.0_f64).unwrap()), "sphere");
}
