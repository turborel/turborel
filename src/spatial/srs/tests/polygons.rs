//! Requirements of the geodetic area and perimeter, against the
//! GeographicLib Planimeter examples of Karney 2013 §6.

use super::super::geodesic::Geodesic;

/// Asserts the (perimeter, area) of `ring` given as [lat, lon] pairs.
fn measures(ring: &[[f64; 2]], perimeter: f64, area: f64) {
    let g = Geodesic::wgs84();
    let lonlat: Vec<[f64; 2]> = ring.iter().map(|[lat, lon]| [*lon, *lat]).collect();
    let (p, a) = g.polygon(&lonlat);
    assert!((p - perimeter).abs() < 0.001_f64 * perimeter.max(1.0_f64) / 1000.0_f64 + 0.0001_f64, "perimeter {p} for {perimeter}");
    assert!((a - area).abs() < 1.0_f64, "area {a} for {area}");
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.41 Description 2a iv] polygons
/// around a pole (an odd number of crossings of the prime meridian),
/// across the equator and of an octant, perimeter to 0.1 mm and area to
/// a square metre; reversing the ring negates the area.
#[test]
fn polygon_area_matches_the_planimeter() {
    measures(
        &[[89.0_f64, 0.0_f64], [89.0_f64, 90.0_f64], [89.0_f64, 180.0_f64], [89.0_f64, 270.0_f64]],
        631_819.874_5_f64,
        24_952_305_678.0_f64,
    );
    measures(
        &[[-89.0_f64, 0.0_f64], [-89.0_f64, 90.0_f64], [-89.0_f64, 180.0_f64], [-89.0_f64, 270.0_f64]],
        631_819.874_5_f64,
        -24_952_305_678.0_f64,
    );
    measures(&[[0.0_f64, -1.0_f64], [-1.0_f64, 0.0_f64], [0.0_f64, 1.0_f64], [1.0_f64, 0.0_f64]], 627_598.273_1_f64, 24_619_419_146.0_f64);
    measures(&[[90.0_f64, 0.0_f64], [0.0_f64, 0.0_f64], [0.0_f64, 90.0_f64]], 30_022_685.0_f64, 63_758_202_715_511.0_f64);
    measures(&[[0.0_f64, 90.0_f64], [0.0_f64, 0.0_f64], [90.0_f64, 0.0_f64]], 30_022_685.0_f64, -63_758_202_715_511.0_f64);
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.41 Description 2a iv] an empty
/// ring and a single point measure zero, and the whole ellipsoid's area
/// is 4π times the authalic radius squared.
#[test]
fn degenerate_polygons_measure_zero() {
    let g = Geodesic::wgs84();
    assert_eq!(g.polygon(&[]), (0.0_f64, 0.0_f64));
    assert_eq!(g.polygon(&[[1.0_f64, 2.0_f64]]), (0.0_f64, 0.0_f64));
    assert!((g.ellipsoid_area() - 510_065_621_724_088.5_f64).abs() < 1.0_f64);
}
