//! The four point-set operations on fixed configurations, among them the
//! degenerate ones of shared edges, touching vertices, collinear overlaps
//! and holes touching shells. Results are compared as point sets and by the
//! type of their value.

use super::super::{Geometry, area, difference, equals, intersection, sym_difference, union};
use super::{text, wkt};

const SQUARE: &str = "POLYGON ((0 0, 2 0, 2 2, 0 2, 0 0))";
const SHIFTED: &str = "POLYGON ((1 1, 3 1, 3 3, 1 3, 1 1))";

/// The result has the expected point set and the same type of value.
fn check(actual: &Geometry, expected: &str) {
    let expected = wkt(expected);
    assert_eq!(std::mem::discriminant(actual), std::mem::discriminant(&expected), "{} is not {}", text(actual), text(&expected));
    assert_eq!(equals(actual, &expected).unwrap(), Some(true), "{} is not {}", text(actual), text(&expected));
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.31, §5.1.33, §5.1.35, §5.1.37]
/// each operation on overlapping polygons, with the return types of
/// §5.1.39 Tables 8 to 11.
#[test]
fn overlapping_polygons() {
    let (square, shifted) = (wkt(SQUARE), wkt(SHIFTED));
    check(&intersection(&square, &shifted).unwrap(), "POLYGON ((1 1, 2 1, 2 2, 1 2, 1 1))");
    check(&union(&square, &shifted).unwrap(), "POLYGON ((0 0, 2 0, 2 1, 3 1, 3 3, 1 3, 1 2, 0 2, 0 0))");
    check(&difference(&square, &shifted).unwrap(), "POLYGON ((0 0, 2 0, 2 1, 1 1, 1 2, 0 2, 0 0))");
    check(
        &sym_difference(&square, &shifted).unwrap(),
        "MULTIPOLYGON (((0 0, 2 0, 2 1, 1 1, 1 2, 0 2, 0 0)), ((2 1, 3 1, 3 3, 1 3, 1 2, 2 2, 2 1)))",
    );
    assert_eq!(area(&union(&square, &shifted).unwrap()), Some(7.0_f64));
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.31, §5.1.33] polygons sharing an
/// edge meet in that edge and unite into one polygon; polygons touching
/// at a corner meet in a point.
#[test]
fn shared_edges_and_touching_vertices() {
    let square = wkt(SQUARE);
    let beside = wkt("POLYGON ((2 0, 4 0, 4 2, 2 2, 2 0))");
    check(&intersection(&square, &beside).unwrap(), "LINESTRING (2 0, 2 2)");
    check(&union(&square, &beside).unwrap(), "POLYGON ((0 0, 4 0, 4 2, 0 2, 0 0))");
    let corner = wkt("POLYGON ((2 2, 4 2, 4 4, 2 4, 2 2))");
    check(&intersection(&square, &corner).unwrap(), "POINT (2 2)");
    check(&union(&square, &corner).unwrap(), "MULTIPOLYGON (((0 0, 2 0, 2 2, 0 2, 0 0)), ((2 2, 4 2, 4 4, 2 4, 2 2)))");
    check(&difference(&square, &corner).unwrap(), SQUARE);
    let partial = wkt("POLYGON ((2 1, 3 1, 3 3, 2 3, 2 1))");
    check(&intersection(&square, &partial).unwrap(), "LINESTRING (2 1, 2 2)");
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.31, §5.1.33, §5.1.35] a line
/// crossing a polygon gives the part inside, the parts outside and a
/// mixed collection for the union.
#[test]
fn lines_and_polygons() {
    let (square, line) = (wkt(SQUARE), wkt("LINESTRING (-1 1, 3 1)"));
    check(&intersection(&line, &square).unwrap(), "LINESTRING (0 1, 2 1)");
    check(&difference(&line, &square).unwrap(), "MULTILINESTRING ((-1 1, 0 1), (2 1, 3 1))");
    check(&difference(&square, &line).unwrap(), SQUARE);
    check(
        &union(&square, &line).unwrap(),
        "GEOMETRYCOLLECTION (POLYGON ((0 0, 2 0, 2 2, 0 2, 0 0)), LINESTRING (-1 1, 0 1), LINESTRING (2 1, 3 1))",
    );
    check(&intersection(&wkt("LINESTRING (0 0, 2 0)"), &square).unwrap(), "LINESTRING (0 0, 2 0)");
    check(&intersection(&wkt("LINESTRING (-1 0, 0 0, 0 -1)"), &square).unwrap(), "POINT (0 0)");
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.31, §5.1.33, §5.1.37] crossing
/// lines meet in a point; collinear overlapping lines meet in their
/// shared stretch and unite into one line string.
#[test]
fn crossing_and_collinear_lines() {
    let (first, second) = (wkt("LINESTRING (0 0, 2 2)"), wkt("LINESTRING (0 2, 2 0)"));
    check(&intersection(&first, &second).unwrap(), "POINT (1 1)");
    check(&union(&first, &second).unwrap(), "MULTILINESTRING ((0 0, 1 1), (1 1, 2 2), (0 2, 1 1), (1 1, 2 0))");
    let (left, right) = (wkt("LINESTRING (0 0, 2 0)"), wkt("LINESTRING (1 0, 3 0)"));
    check(&intersection(&left, &right).unwrap(), "LINESTRING (1 0, 2 0)");
    check(&union(&left, &right).unwrap(), "LINESTRING (0 0, 3 0)");
    check(&sym_difference(&left, &right).unwrap(), "MULTILINESTRING ((0 0, 1 0), (2 0, 3 0))");
    let thirds = intersection(&wkt("LINESTRING (0 0, 3 1)"), &wkt("LINESTRING (0 1, 3 0)")).unwrap();
    let Geometry::Point([x, y]) = thirds else { panic!("{}", text(&thirds)) };
    assert!((x - 1.5).abs() < 1e-15_f64 && (y - 0.5).abs() < 1e-15_f64);
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.31, §5.1.35, §5.1.39] points
/// against surfaces and other points, and the empty set of type
/// ST_Point when nothing remains (Table 7 R00).
#[test]
fn points() {
    let square = wkt(SQUARE);
    check(&intersection(&wkt("POINT (1 1)"), &square).unwrap(), "POINT (1 1)");
    check(&intersection(&wkt("POINT (0 1)"), &square).unwrap(), "POINT (0 1)");
    assert_eq!(intersection(&wkt("POINT (3 1)"), &square).unwrap(), Geometry::Empty);
    assert_eq!(difference(&wkt("POINT (1 1)"), &square).unwrap(), Geometry::Empty);
    check(&union(&wkt("MULTIPOINT ((0 0), (1 1))"), &wkt("POINT (5 5)")).unwrap(), "MULTIPOINT ((0 0), (1 1), (5 5))");
    check(&union(&wkt("POINT (1 1)"), &square).unwrap(), SQUARE);
    check(&union(&wkt("POINT (3 3)"), &square).unwrap(), "GEOMETRYCOLLECTION (POLYGON ((0 0, 2 0, 2 2, 0 2, 0 0)), POINT (3 3))");
    check(&sym_difference(&wkt("MULTIPOINT ((0 0), (1 1))"), &wkt("MULTIPOINT ((1 1), (2 2))")).unwrap(), "MULTIPOINT ((0 0), (2 2))");
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.35] a difference cutting a hole,
/// and one leaving a hole that touches the exterior ring at a point.
#[test]
fn holes_from_differences() {
    let big = wkt("POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))");
    check(
        &difference(&big, &wkt("POLYGON ((1 1, 3 1, 3 3, 1 3, 1 1))")).unwrap(),
        "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0), (1 1, 1 3, 3 3, 3 1, 1 1))",
    );
    let touching = difference(&big, &wkt("POLYGON ((0 2, 2 1, 2 3, 0 2))")).unwrap();
    let Geometry::Polygon(rings) = &touching else { panic!("{}", text(&touching)) };
    assert_eq!(rings.len(), 2, "{}", text(&touching));
    assert_eq!(area(&touching), Some(14.0_f64));
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.31, §5.1.33, §5.1.35, §5.1.37]
/// identical operands and empty operands.
#[test]
fn identical_and_empty_operands() {
    let square = wkt(SQUARE);
    let same = wkt("POLYGON ((2 2, 0 2, 0 0, 2 0, 2 2))");
    check(&intersection(&square, &same).unwrap(), SQUARE);
    check(&union(&square, &same).unwrap(), SQUARE);
    assert_eq!(difference(&square, &same).unwrap(), Geometry::Empty);
    assert_eq!(sym_difference(&square, &same).unwrap(), Geometry::Empty);
    let empty = wkt("POLYGON EMPTY");
    assert_eq!(intersection(&square, &empty).unwrap(), Geometry::Empty);
    assert_eq!(union(&empty, &square).unwrap(), square);
    assert_eq!(difference(&square, &empty).unwrap(), square);
    assert_eq!(difference(&empty, &square).unwrap(), Geometry::Empty);
    assert_eq!(sym_difference(&empty, &square).unwrap(), square);
}
