//! The DE-9IM and the named relationships on fixed configurations of
//! every dimension pair, degenerate ones included.

use super::super::{Matrix, contains, crosses, disjoint, equals, intersects, overlaps, relate, relate_pattern, touches, within};
use super::wkt;

const SQUARE: &str = "POLYGON ((0 0, 2 0, 2 2, 0 2, 0 0))";

fn matrix(first: &str, second: &str) -> String {
    relate(&wkt(first), &wkt(second)).unwrap().map_or_else(|| "null".to_owned(), |matrix: Matrix| matrix.code())
}

/// Verifies: [ISO/IEC 13249-3:2016 §4.2.2.1] Table 5's nine cells for
/// each pair of dimensions, with the dimension of each intersection.
#[test]
fn matrices_of_every_dimension_pair() {
    let cases = [
        (SQUARE, "POLYGON ((1 1, 3 1, 3 3, 1 3, 1 1))", "212101212"),
        (SQUARE, "POLYGON ((2 0, 4 0, 4 2, 2 2, 2 0))", "FF2F11212"),
        (SQUARE, "POLYGON ((2 2, 4 2, 4 4, 2 4, 2 2))", "FF2F01212"),
        (SQUARE, "POLYGON ((5 5, 6 5, 6 6, 5 5))", "FF2FF1212"),
        ("POINT (1 1)", SQUARE, "0FFFFF212"),
        ("POINT (0 1)", SQUARE, "F0FFFF212"),
        ("POINT (3 1)", SQUARE, "FF0FFF212"),
        ("LINESTRING (-1 1, 3 1)", SQUARE, "101FF0212"),
        ("LINESTRING (0.5 0.5, 1.5 1.5)", SQUARE, "1FF0FF212"),
        ("LINESTRING (0 0, 2 0)", SQUARE, "F1FF0F212"),
        ("LINESTRING (0 0, 2 2)", "LINESTRING (0 2, 2 0)", "0F1FF0102"),
        ("LINESTRING (0 0, 2 0)", "LINESTRING (1 0, 3 0)", "1010F0102"),
        ("LINESTRING (0 0, 2 0)", "LINESTRING (1 0, 1 1)", "F01FF0102"),
        ("POINT (1 1)", "POINT (1 1)", "0FFFFFFF2"),
        ("MULTIPOINT ((1 1), (2 2))", "POINT (1 1)", "0F0FFFFF2"),
    ];
    for (first, second, expected) in cases {
        assert_eq!(matrix(first, second), expected, "{first} / {second}");
        let transposed = relate(&wkt(second), &wkt(first)).unwrap().unwrap().transposed().code();
        assert_eq!(transposed, expected, "transposed {first} / {second}");
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 §4.2.2] the boundary of a multicurve
/// is the mod 2 union of its elements' ends, so a shared end is interior
/// and a closed curve has no boundary.
#[test]
fn multicurve_boundary_is_the_mod_2_union() {
    assert_eq!(matrix("POINT (1 0)", "MULTILINESTRING ((0 0, 1 0), (1 0, 2 0))"), "0FFFFF102");
    assert_eq!(matrix("POINT (0 0)", "MULTILINESTRING ((0 0, 1 0), (1 0, 2 0))"), "F0FFFF102");
    assert_eq!(matrix("POINT (0 0)", "LINESTRING (0 0, 1 0, 1 1, 0 0)"), "0FFFFF1F2");
    assert_eq!(matrix("POINT (1 0)", "MULTILINESTRING ((0 0, 1 0), (1 0, 2 0), (1 0, 1 1))"), "F0FFFF102");
}

/// Verifies: [ISO/IEC 13249-3:2016 §4.2.2.1] the boundaries of two
/// polygons may meet in a point and a line string at once, so their
/// boundary cell is 1; a line's interior meets a polygon's interior only
/// in dimension 1 and two polygons' interiors only in dimension 2.
#[test]
fn worked_examples_of_4_2_2_1() {
    let notched = "POLYGON ((2 0, 4 0, 4 3, 3 3, 2 2, 2 1, 2 0))";
    let first = relate(&wkt("POLYGON ((0 0, 2 0, 2 1, 1 2, 2 2, 2 3, 0 3, 0 0))"), &wkt(notched)).unwrap().unwrap();
    assert_eq!(first.code(), "FF2F11212");
    assert_eq!(matrix("LINESTRING (-1 1, 3 1)", SQUARE).chars().next(), Some('1'));
    assert_eq!(matrix(SQUARE, "POLYGON ((1 1, 3 1, 3 3, 1 3, 1 1))").chars().next(), Some('2'));
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.45] ST_Relate matches each cell
/// against T, F, 0, 1, 2 and *, refuses any other pattern character, and
/// is the null value for an empty operand.
#[test]
fn relate_matches_patterns() {
    let square = wkt(SQUARE);
    let inner = wkt("POINT (1 1)");
    assert_eq!(relate_pattern(&inner, &square, "0FFFFF212").unwrap(), Some(true));
    assert_eq!(relate_pattern(&inner, &square, "T*F**F***").unwrap(), Some(true));
    assert_eq!(relate_pattern(&inner, &square, "1********").unwrap(), Some(false));
    assert_eq!(relate_pattern(&inner, &square, "*T*******").unwrap(), Some(false));
    assert_eq!(relate_pattern(&inner, &square, "*********").unwrap(), Some(true));
    assert_eq!(relate_pattern(&wkt("POINT EMPTY"), &square, "*********").unwrap(), None);
    for bad in ["0FFFFF21", "0FFFFF2122", "0FFFFF21x", "t********"] {
        let error = relate_pattern(&inner, &square, bad).unwrap_err();
        assert!(error.msg.contains("invalid intersection matrix"), "{bad}");
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.43, §5.1.46, §5.1.48, §5.1.52,
/// §5.1.53] equality ignores representation, disjoint and intersects are
/// complements, and within and contains are converses.
#[test]
fn equals_disjoint_within_and_contains() {
    let square = wkt(SQUARE);
    let rotated = wkt("POLYGON ((2 2, 0 2, 0 0, 2 0, 2 2))");
    let split = wkt("POLYGON ((0 0, 1 0, 2 0, 2 2, 0 2, 0 0))");
    assert_eq!(equals(&square, &rotated).unwrap(), Some(true));
    assert_eq!(equals(&square, &split).unwrap(), Some(true));
    assert_eq!(equals(&wkt("LINESTRING (0 0, 2 0)"), &wkt("MULTILINESTRING ((2 0, 1 0), (0 0, 1 0))")).unwrap(), Some(true));
    assert_eq!(equals(&wkt("POINT EMPTY"), &wkt("LINESTRING EMPTY")).unwrap(), Some(true));
    assert_eq!(equals(&wkt("POINT EMPTY"), &square).unwrap(), Some(false));
    let far = wkt("POINT (5 5)");
    assert_eq!(disjoint(&square, &far).unwrap(), Some(true));
    assert_eq!(intersects(&square, &far).unwrap(), Some(false));
    assert_eq!(intersects(&square, &wkt("POINT (2 2)")).unwrap(), Some(true));
    assert_eq!(within(&wkt("POINT (1 1)"), &square).unwrap(), Some(true));
    assert_eq!(within(&wkt("POINT (2 1)"), &square).unwrap(), Some(false));
    assert_eq!(contains(&square, &wkt("LINESTRING (0 0, 1 1)")).unwrap(), Some(true));
    assert_eq!(contains(&square, &wkt("LINESTRING (0 0, 2 0)")).unwrap(), Some(false));
    assert_eq!(disjoint(&square, &wkt("POINT EMPTY")).unwrap(), None);
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.50] touches is the null value
/// for two point sets and otherwise holds when the operands meet only on
/// a boundary.
#[test]
fn touches_by_dimension() {
    let square = wkt(SQUARE);
    assert_eq!(touches(&wkt("POINT (1 1)"), &wkt("POINT (1 1)")).unwrap(), None);
    assert_eq!(touches(&wkt("POINT (0 1)"), &square).unwrap(), Some(true));
    assert_eq!(touches(&wkt("POINT (1 1)"), &square).unwrap(), Some(false));
    assert_eq!(touches(&square, &wkt("POLYGON ((2 2, 3 2, 3 3, 2 2))")).unwrap(), Some(true));
    assert_eq!(touches(&wkt("LINESTRING (0 0, 1 0)"), &wkt("LINESTRING (1 0, 2 1)")).unwrap(), Some(true));
    assert_eq!(touches(&wkt("LINESTRING (0 0, 2 0)"), &wkt("LINESTRING (1 0, 2 1)")).unwrap(), Some(true));
    assert_eq!(touches(&wkt("LINESTRING (0 0, 2 0)"), &wkt("LINESTRING (1 -1, 1 1)")).unwrap(), Some(false));
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.51] crosses by each dimension
/// pair it defines, and the null value for the others.
#[test]
fn crosses_by_dimension() {
    let square = wkt(SQUARE);
    assert_eq!(crosses(&wkt("MULTIPOINT ((1 1), (3 3))"), &square).unwrap(), Some(true));
    assert_eq!(crosses(&wkt("MULTIPOINT ((1 1), (1.5 1.5))"), &square).unwrap(), Some(false));
    assert_eq!(crosses(&wkt("MULTIPOINT ((1 0), (5 5))"), &wkt("LINESTRING (0 0, 2 0)")).unwrap(), Some(true));
    assert_eq!(crosses(&wkt("LINESTRING (-1 1, 3 1)"), &square).unwrap(), Some(true));
    assert_eq!(crosses(&wkt("LINESTRING (0 0, 2 2)"), &wkt("LINESTRING (0 2, 2 0)")).unwrap(), Some(true));
    assert_eq!(crosses(&wkt("LINESTRING (0 0, 2 0)"), &wkt("LINESTRING (1 0, 3 0)")).unwrap(), Some(false));
    assert_eq!(crosses(&square, &wkt("LINESTRING (-1 1, 3 1)")).unwrap(), None);
    assert_eq!(crosses(&wkt("LINESTRING (0 0, 2 0)"), &wkt("POINT (1 0)")).unwrap(), None);
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.54] overlaps for two point sets,
/// two curves and two surfaces, and the null value across dimensions.
#[test]
fn overlaps_by_dimension() {
    let square = wkt(SQUARE);
    assert_eq!(overlaps(&square, &wkt("POLYGON ((1 1, 3 1, 3 3, 1 3, 1 1))")).unwrap(), Some(true));
    assert_eq!(overlaps(&square, &wkt("POLYGON ((0.5 0.5, 1 0.5, 1 1, 0.5 0.5))")).unwrap(), Some(false));
    assert_eq!(overlaps(&wkt("LINESTRING (0 0, 2 0)"), &wkt("LINESTRING (1 0, 3 0)")).unwrap(), Some(true));
    assert_eq!(overlaps(&wkt("LINESTRING (0 0, 2 2)"), &wkt("LINESTRING (0 2, 2 0)")).unwrap(), Some(false));
    assert_eq!(overlaps(&wkt("MULTIPOINT ((0 0), (1 1))"), &wkt("MULTIPOINT ((1 1), (2 2))")).unwrap(), Some(true));
    assert_eq!(overlaps(&square, &wkt("LINESTRING (1 0, 3 0)")).unwrap(), None);
}

/// Verifies: [ISO/IEC 13249-3:2016 §4.2.2.1] a polygon's hole is in its
/// exterior, a hole touching the shell keeps the interior's cells, and a
/// collection's polygons decide over its lines.
#[test]
fn holes_and_collections() {
    let holed = "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0), (1 1, 3 1, 3 3, 1 3, 1 1))";
    assert_eq!(matrix("POINT (2 2)", holed), "FF0FFF212");
    assert_eq!(matrix("POINT (1 2)", holed), "F0FFFF212");
    let touching = "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0), (0 2, 2 1, 2 3, 0 2))";
    assert_eq!(matrix("POINT (0 2)", touching), "F0FFFF212");
    assert_eq!(matrix("POINT (1 2)", touching), "FF0FFF212");
    let collection = "GEOMETRYCOLLECTION (POLYGON ((0 0, 2 0, 2 2, 0 2, 0 0)), LINESTRING (1 1, 3 1))";
    assert_eq!(matrix("POINT (1 1)", collection), "0FFFFF212");
    assert_eq!(matrix("POINT (3 1)", collection), "F0FFFF212");
    assert_eq!(matrix("POINT (2.5 1)", collection), "0FFFFF212");
}
