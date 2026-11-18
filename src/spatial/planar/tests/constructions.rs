//! Buffer, convex hull, boundary and polygons from rings.

use super::super::{Geometry, area, boundary, buffer, convex_hull, equals, multipolygon_from_rings, polygon_from_rings, within};
use super::{text, wkt};

fn same_set(actual: &Geometry, expected: &str) {
    assert_eq!(equals(actual, &wkt(expected)).unwrap(), Some(true), "{} is not {expected}", text(actual));
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.30] a point's buffer is a disc,
/// inscribed in the circle and close to its area; a buffer contains its
/// geometry.
#[test]
fn buffer_of_a_point() {
    let disc = buffer(&wkt("POINT (1 1)"), 2.0).unwrap();
    let Geometry::Polygon(rings) = &disc else { panic!("{}", text(&disc)) };
    assert_eq!(rings.first().map(Vec::len), Some(33));
    let measured = area(&disc).unwrap();
    let circle = std::f64::consts::PI * 4.0_f64;
    assert!(measured < circle && measured > circle * 0.99_f64, "{measured}");
    assert_eq!(within(&wkt("POINT (1 1)"), &disc).unwrap(), Some(true));
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.30] a line's buffer is one
/// surface around it, with round ends and an outer corner; a polygon's grows outward and keeps its hole when
/// the distance is small and fills it when large.
#[test]
fn buffer_of_lines_and_polygons() {
    let line = wkt("LINESTRING (0 0, 10 0, 10 10)");
    let around = buffer(&line, 1.0).unwrap();
    assert!(matches!(around, Geometry::Polygon(_)), "{}", text(&around));
    assert_eq!(within(&line, &around).unwrap(), Some(true));
    let measured = area(&around).unwrap();
    let exact = 39.0_f64 + 1.25_f64 * std::f64::consts::PI;
    assert!(measured < exact && measured > exact - 0.05_f64, "{measured}");
    let holed = wkt("POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (4 4, 6 4, 6 6, 4 6, 4 4))");
    let grown = buffer(&holed, 0.5).unwrap();
    let Geometry::Polygon(rings) = &grown else { panic!("{}", text(&grown)) };
    assert_eq!(rings.len(), 2, "{}", text(&grown));
    let filled = buffer(&holed, 2.0).unwrap();
    let Geometry::Polygon(filled_rings) = &filled else { panic!("{}", text(&filled)) };
    assert_eq!(filled_rings.len(), 1, "{}", text(&filled));
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.30] the buffer at distance zero
/// is the geometry, below zero the empty set, and a distance that is not
/// a number is refused.
#[test]
fn buffer_distances() {
    let square = wkt("POLYGON ((0 0, 2 0, 2 2, 0 2, 0 0))");
    assert_eq!(buffer(&square, 0.0).unwrap(), square);
    assert_eq!(buffer(&square, -1.0).unwrap(), Geometry::Empty);
    assert_eq!(buffer(&wkt("POINT EMPTY"), 1.0).unwrap(), Geometry::Empty);
    assert!(buffer(&square, f64::NAN).is_err());
    assert!(buffer(&square, f64::INFINITY).is_err());
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.29] the convex hull drops inner
/// and collinear points and degenerates to a line or a point.
#[test]
fn convex_hulls() {
    let hull = convex_hull(&wkt("MULTIPOINT ((0 0), (2 0), (1 1), (2 2), (0 2), (1 0))"));
    assert_eq!(text(&hull), "POLYGON ((0 0, 2 0, 2 2, 0 2, 0 0))");
    assert_eq!(text(&convex_hull(&wkt("LINESTRING (0 0, 1 1, 3 3, 2 2)"))), "LINESTRING (0 0, 3 3)");
    assert_eq!(text(&convex_hull(&wkt("MULTIPOINT ((1 1), (1 1))"))), "POINT (1 1)");
    assert_eq!(convex_hull(&wkt("POINT EMPTY")), Geometry::Empty);
    same_set(&convex_hull(&wkt("POLYGON ((0 0, 4 0, 2 1, 4 4, 0 4, 0 0))")), "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))");
}

/// Verifies: [ISO/IEC 13249-3:2016 §4.2.2, §5.1.17] boundaries: none for
/// points and closed curves, the ends of an open curve, the mod 2 ends
/// of a multicurve (the worked example), the rings of a polygon, and a
/// collection's through its graph.
#[test]
fn boundaries() {
    assert_eq!(boundary(&wkt("POINT (1 1)")).unwrap(), Geometry::Empty);
    assert_eq!(text(&boundary(&wkt("LINESTRING (0 0, 1 0, 1 1)")).unwrap()), "MULTIPOINT (0 0, 1 1)");
    assert_eq!(boundary(&wkt("LINESTRING (0 0, 1 0, 1 1, 0 0)")).unwrap(), Geometry::Empty);
    let multi = wkt("MULTILINESTRING ((0 0, 1 0), (1 0, 2 0), (1 0, 1 1))");
    assert_eq!(text(&boundary(&multi).unwrap()), "MULTIPOINT (0 0, 1 0, 2 0, 1 1)");
    let chained = wkt("MULTILINESTRING ((0 0, 1 0), (1 0, 2 0))");
    assert_eq!(text(&boundary(&chained).unwrap()), "MULTIPOINT (0 0, 2 0)");
    let holed = wkt("POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0), (1 1, 1 2, 2 2, 1 1))");
    assert_eq!(text(&boundary(&holed).unwrap()), "MULTILINESTRING ((0 0, 4 0, 4 4, 0 4, 0 0), (1 1, 1 2, 2 2, 1 1))");
    let collection = wkt("GEOMETRYCOLLECTION (POLYGON ((0 0, 2 0, 2 2, 0 2, 0 0)), LINESTRING (1 1, 3 1))");
    same_set(&boundary(&collection).unwrap(), "GEOMETRYCOLLECTION (LINESTRING (0 0, 2 0, 2 2, 0 2, 0 0), POINT (3 1))");
}

/// Verifies: [ISO/IEC 13249-3:2016 §8.3.9, §10.6.7] rings become
/// polygons by nesting, whatever their order and direction; a ring that
/// is not a linear ring, or rings that are not one polygon for
/// ST_BdPolyFromText, are an invalid argument.
#[test]
fn polygons_from_rings() {
    let outer = wkt("LINESTRING (0 0, 0 4, 4 4, 4 0, 0 0)");
    let inner = wkt("LINESTRING (1 1, 3 1, 3 3, 1 3, 1 1)");
    let island = wkt("LINESTRING (10 10, 11 10, 11 11, 10 10)");
    let ring = |geometry: &Geometry| if let Geometry::Line(points) = geometry { points.clone() } else { Vec::new() };
    let polygon = polygon_from_rings(&[ring(&inner), ring(&outer)]).unwrap();
    same_set(&polygon, "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0), (1 1, 1 3, 3 3, 3 1, 1 1))");
    let many = multipolygon_from_rings(&[ring(&outer), ring(&island), ring(&inner)]).unwrap();
    assert!(matches!(&many, Geometry::MultiPolygon(polygons) if polygons.len() == 2), "{}", text(&many));
    let single = multipolygon_from_rings(&[ring(&outer)]).unwrap();
    assert!(matches!(&single, Geometry::MultiPolygon(polygons) if polygons.len() == 1));
    let error = polygon_from_rings(&[ring(&outer), ring(&island)]).unwrap_err();
    assert!(error.msg.contains("invalid argument"));
    let open = vec![[0.0_f64, 0.0_f64], [1.0_f64, 0.0_f64], [1.0_f64, 1.0_f64], [0.0_f64, 1.0_f64]];
    assert!(polygon_from_rings(&[open]).unwrap_err().msg.contains("invalid argument"));
    let bow = vec![[0.0_f64, 0.0_f64], [2.0_f64, 2.0_f64], [2.0_f64, 0.0_f64], [0.0_f64, 2.0_f64], [0.0_f64, 0.0_f64]];
    assert!(multipolygon_from_rings(&[bow]).is_err());
}

/// Verifies: [ISO/IEC 13249-3:2016 §3.1.2.41 Note 1, §4.2.2] the mod 2
/// union rule's worked example. Curve ends at 10 once, 20 twice, 30
/// three times and 40 four times leave the boundary {10, 30}.
#[test]
fn mod_2_union_rule_example() {
    let curves =
        wkt("MULTILINESTRING ((10 0, 20 20, 30 0), (30 0, 35 5, 40 0), (30 0, 35 -5, 40 0), (20 0, 30 10, 40 0), (20 0, 30 -10, 40 0))");
    assert_eq!(text(&boundary(&curves).unwrap()), "MULTIPOINT (10 0, 30 0)");
}
