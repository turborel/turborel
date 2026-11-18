//! Validity, simplicity, measures and distance.

use super::super::{Reason, area, centroid, distance, is_simple, is_valid, length, perimeter, point_on_surface, validate, within};
use super::wkt;

fn reason(text: &str) -> Option<Reason> {
    validate(&wkt(text)).unwrap().map(|found| found.reason)
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.10, §8.2.1] well formed
/// polygons, a hole touching the shell at one point included, and each
/// malformation with its reason.
#[test]
fn polygon_validity() {
    assert!(is_valid(&wkt("POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0), (1 1, 3 1, 3 3, 1 3, 1 1))")).unwrap());
    assert!(is_valid(&wkt("POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0), (0 2, 2 1, 2 3, 0 2))")).unwrap());
    let cases = [
        ("POLYGON ((0 0, 2 2, 2 0, 0 2, 0 0))", Reason::RingSelfIntersection),
        ("POLYGON ((0 0, 2 0, 0 0))", Reason::TooFewPoints),
        ("POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0), (1 1, 5 1, 5 3, 1 3, 1 1))", Reason::RingsCross),
        ("POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0), (0 1, 2 1, 2 3, 0 3, 0 1))", Reason::RingsCross),
        ("POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0), (0 1, 2 2, 0 3, 1 2, 0 1))", Reason::RingsTouchTwice),
        ("POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0), (5 5, 6 5, 6 6, 5 5))", Reason::HoleOutsideShell),
        ("POLYGON ((0 0, 9 0, 9 9, 0 9, 0 0), (1 1, 8 1, 8 8, 1 8, 1 1), (2 2, 3 2, 3 3, 2 2))", Reason::NestedHoles),
        ("POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0), (2 0, 3 2, 2 4, 1 2, 2 0))", Reason::RingsTouchTwice),
        ("POLYGON ((0 0, 9 0, 9 9, 0 9, 0 0), (1 1, 3 1, 3 3, 1 1), (3 1, 5 1, 3 3, 3 1))", Reason::RingsCross),
    ];
    for (text, expected) in cases {
        assert_eq!(reason(text), Some(expected), "{text}");
    }
    let chained = "POLYGON ((0 0, 9 0, 9 9, 0 9, 0 0), (1 1, 3 1, 2 2, 1 1), (3 1, 5 1, 4 2, 3 1), (2 2, 4 2, 3 3, 2 2))";
    assert_eq!(reason(chained), Some(Reason::DisconnectedInterior));
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.10, §10.6.1] multipolygons
/// whose polygons touch at points are well formed; overlapping ones or
/// ones sharing an edge are not; a line string of one point is not.
#[test]
fn collection_validity() {
    assert!(is_valid(&wkt("MULTIPOLYGON (((0 0, 1 0, 1 1, 0 1, 0 0)), ((1 1, 2 1, 2 2, 1 2, 1 1)))")).unwrap());
    assert_eq!(reason("MULTIPOLYGON (((0 0, 2 0, 2 2, 0 2, 0 0)), ((1 1, 3 1, 3 3, 1 3, 1 1)))"), Some(Reason::OverlappingPolygons));
    assert_eq!(reason("MULTIPOLYGON (((0 0, 1 0, 1 1, 0 1, 0 0)), ((1 0, 2 0, 2 1, 1 1, 1 0)))"), Some(Reason::SharedBoundary));
    assert_eq!(reason("LINESTRING (1 1)"), Some(Reason::TooFewPoints));
    assert!(is_valid(&wkt("GEOMETRYCOLLECTION (POINT (1 1), LINESTRING (0 0, 1 1))")).unwrap());
    assert_eq!(Reason::RingsCross.text(), "rings cross");
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.8, §7.1.1, §10.1.1, §10.2.1,
/// §10.3.1] simplicity of each type.
#[test]
fn simplicity() {
    let cases = [
        ("POINT (1 1)", true),
        ("MULTIPOINT ((1 1), (2 2))", true),
        ("MULTIPOINT ((1 1), (1 1))", false),
        ("LINESTRING (0 0, 1 0, 1 1)", true),
        ("LINESTRING (0 0, 1 0, 1 0, 1 1)", true),
        ("LINESTRING (0 0, 2 0, 2 2, 0 2, 0 0)", true),
        ("LINESTRING (0 0, 2 2, 2 0, 0 2)", false),
        ("LINESTRING (0 0, 2 0, 1 0)", false),
        ("LINESTRING (0 0, 2 0, 2 2, 0 2, 1 0)", false),
        ("LINESTRING (0 0, 2 0, 2 2, 0 2, 0 0, 0 -1)", false),
        ("MULTILINESTRING ((0 0, 1 0), (1 0, 2 0))", true),
        ("MULTILINESTRING ((0 0, 2 0), (1 0, 1 1))", false),
        ("MULTILINESTRING ((0 0, 2 0, 2 2, 0 2, 0 0), (0 0, -1 -1))", false),
        ("POLYGON ((0 0, 2 0, 2 2, 0 2, 0 0))", true),
        ("GEOMETRYCOLLECTION (POINT (5 5), LINESTRING (0 0, 1 1))", true),
        ("GEOMETRYCOLLECTION (POLYGON ((0 0, 2 0, 2 2, 0 2, 0 0)), POINT (1 1))", false),
    ];
    for (text, expected) in cases {
        assert_eq!(is_simple(&wkt(text)).unwrap(), expected, "{text}");
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 §7.1.2, §8.1.2, §8.1.4, §10.3.5,
/// §10.5.3, §10.5.5] Euclidean length, area net of holes and perimeter
/// including the holes' rings; the null value for empty geometries.
#[test]
fn lengths_and_areas() {
    let holed = wkt("POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0), (1 1, 1 2, 2 2, 2 1, 1 1))");
    assert_eq!(area(&holed), Some(15.0_f64));
    assert_eq!(perimeter(&holed), Some(20.0_f64));
    assert_eq!(length(&wkt("MULTILINESTRING ((0 0, 3 4), (0 0, 0 1))")), Some(6.0_f64));
    assert_eq!(length(&holed), Some(0.0_f64));
    assert_eq!(area(&wkt("LINESTRING (0 0, 1 1)")), Some(0.0_f64));
    assert_eq!(area(&wkt("POLYGON EMPTY")), None);
    assert_eq!(length(&wkt("LINESTRING (1e300 0, -1e300 0)")), Some(2e300_f64));
}

/// Verifies: [ISO/IEC 13249-3:2016 §8.1.6, §8.1.8, §10.5.7, §10.5.9] the
/// centroid of surfaces with holes, none without a surface, and a point on the surface that lies
/// in its interior even where the centroid does not.
#[test]
fn centroids_and_points_on_surfaces() {
    assert_eq!(centroid(&wkt("POLYGON ((0 0, 4 0, 4 2, 0 2, 0 0))")), Some([2.0_f64, 1.0_f64]));
    let [x, y] = centroid(&wkt("POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0), (2 0, 4 0, 4 4, 2 4, 2 0))")).unwrap_or_default();
    assert!((x - 1.0).abs() < 1e-12_f64 && (y - 2.0).abs() < 1e-12_f64);
    assert_eq!(centroid(&wkt("LINESTRING (0 0, 2 0)")), None);
    assert_eq!(centroid(&wkt("MULTIPOINT ((0 0), (2 2))")), None);
    let shape = wkt("POLYGON ((0 0, 6 0, 6 6, 5 6, 5 1, 1 1, 1 6, 0 6, 0 0))");
    let point = point_on_surface(&shape).unwrap();
    assert_eq!(within(&super::super::Geometry::Point(point), &shape).unwrap(), Some(true));
    assert_eq!(point_on_surface(&wkt("POINT (3 4)")), Some([3.0_f64, 4.0_f64]));
    assert_eq!(point_on_surface(&wkt("POINT EMPTY")), None);
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.41] distance between each pair
/// of types, zero for meeting or nested operands, the null value for an
/// empty operand.
#[test]
fn distances() {
    let square = wkt("POLYGON ((0 0, 2 0, 2 2, 0 2, 0 0))");
    assert_eq!(distance(&wkt("POINT (5 1)"), &square), Some(3.0_f64));
    assert_eq!(distance(&wkt("POINT (5 6)"), &square), Some(5.0_f64));
    assert_eq!(distance(&wkt("POINT (1 1)"), &square), Some(0.0_f64));
    assert_eq!(distance(&wkt("POLYGON ((0.5 0.5, 1 0.5, 1 1, 0.5 0.5))"), &square), Some(0.0_f64));
    assert_eq!(distance(&wkt("LINESTRING (3 -5, 3 5)"), &square), Some(1.0_f64));
    assert_eq!(distance(&wkt("LINESTRING (0 3, 4 7)"), &wkt("LINESTRING (4 3, 8 7)")), Some(8.0_f64.sqrt()));
    assert_eq!(distance(&wkt("POINT (0 0)"), &wkt("POINT (3 4)")), Some(5.0_f64));
    let holed = wkt("POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (2 2, 8 2, 8 8, 2 8, 2 2))");
    assert_eq!(distance(&wkt("POINT (5 5)"), &holed), Some(3.0_f64));
    assert_eq!(distance(&wkt("POINT EMPTY"), &square), None);
}
