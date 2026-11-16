//! Closed-set spatial relationships, including holes and exact boundaries.

use super::*;

/// Verifies: [EXTENSION] D82; OGC 06-103r4 §6.1.2.3: intersections include
/// boundaries, exclude hole interiors and are symmetric for all seven
/// geometry kinds. Empty components cannot introduce a shared point.
#[test]
fn intersections_use_shapes_instead_of_envelopes() {
    let cases = [
        ("POINT (1 1)", "POINT (1 1)", true),
        ("POINT (1 1)", "POINT (1 2)", false),
        ("POINT EMPTY", "POINT EMPTY", false),
        ("POINT (0 0)", "LINESTRING (0 0, 2 2)", true),
        ("POINT (1 1)", "LINESTRING (0 0, 2 2)", true),
        ("POINT (1 1.0000000000000002)", "LINESTRING (0 0, 2 2)", false),
        ("POINT (1 1)", "LINESTRING (1 1, 1 1)", true),
        ("LINESTRING (0 0, 2 2)", "LINESTRING (0 2, 2 0)", true),
        ("LINESTRING (0 0, 2 2)", "LINESTRING (3 3, 4 4)", false),
        ("LINESTRING (0 0, 2 2)", "LINESTRING (1 1, 3 3)", true),
        ("LINESTRING (0 0, 2 2)", "LINESTRING (0 1, 1 2)", false),
        ("LINESTRING (1 1, 1 1)", "LINESTRING (0 0, 2 2)", true),
        ("POINT (1 1)", "MULTIPOINT (EMPTY, (1 1), (2 2))", true),
        ("POINT (1 1)", "MULTILINESTRING (EMPTY, (0 0, 2 2))", true),
        ("LINESTRING EMPTY", "MULTILINESTRING (EMPTY, (0 0, 2 2))", false),
        ("POINT (1 1)", "MULTIPOLYGON (EMPTY, ((0 0, 2 0, 2 2, 0 2, 0 0)))", true),
        ("POINT (9 9)", "GEOMETRYCOLLECTION (POINT EMPTY, GEOMETRYCOLLECTION (POINT (9 9)))", true),
        ("MULTIPOINT EMPTY", "GEOMETRYCOLLECTION (LINESTRING EMPTY, POLYGON EMPTY)", false),
    ];
    for (left, right, expected) in cases {
        intersection(left, right, expected);
    }
}

fn intersection(left: &str, right: &str, expected: bool) {
    let a = Geometry::from_wkt(left, 4326).unwrap();
    let b = Geometry::from_wkt(right, 4326).unwrap();
    assert_eq!(a.intersects(&b).unwrap(), expected, "{left} / {right}");
    assert_eq!(b.intersects(&a).unwrap(), expected, "{right} / {left}");
}

/// Verifies: [EXTENSION] D82; OGC 06-103r4 §6.1.11.1: both exterior
/// and interior rings bound a closed polygon. A hole interior is outside;
/// tangency, shared edges and containment still count as intersection.
#[test]
fn polygon_holes_boundaries_and_containment_are_distinct() {
    let polygon = "POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (2 2, 8 2, 8 8, 2 8, 2 2))";
    for (shape, expected) in [
        ("POINT (0 0)", true),
        ("POINT (1 1)", true),
        ("POINT (5 5)", false),
        ("POINT (2 5)", true),
        ("POINT (11 5)", false),
        ("LINESTRING (3 3, 7 7)", false),
        ("LINESTRING (1 1, 9 9)", true),
        ("LINESTRING (1 1, 1 9)", true),
        ("LINESTRING (10 0, 10 10)", true),
        ("LINESTRING (5 5, 20 5)", true),
        ("POLYGON ((3 3, 7 3, 7 7, 3 7, 3 3))", false),
        ("POLYGON ((0.1 0.1, 1 0.1, 1 1, 0.1 1, 0.1 0.1))", true),
        ("POLYGON ((10 10, 11 10, 11 11, 10 11, 10 10))", true),
        ("POLYGON ((-1 -1, 11 -1, 11 11, -1 11, -1 -1))", true),
        ("POLYGON EMPTY", false),
    ] {
        intersection(polygon, shape, expected);
    }
    intersection("POLYGON ((0 0, 0 2, 2 2, 2 0, 0 0))", "POINT (1 1)", true);
    intersection("POLYGON ((0 0, 2 0, 2 2, 0 2, 0 0))", "POINT (2 1)", true);
}

/// Verifies: [EXTENSION] D82: the reference check precedes envelope and
/// empty-set shortcuts; assigning different reference labels never silently
/// compares coordinates in incompatible spatial reference systems.
#[test]
fn relationships_refuse_mixed_references_even_for_empty_sets() {
    for text in ["POINT EMPTY", "POINT (0 0)"] {
        let a = Geometry::from_wkt(text, 0).unwrap();
        let b = Geometry::from_wkt("POINT EMPTY", 4326).unwrap();
        assert_eq!(a.intersects(&b).unwrap_err().code, Code::Data);
    }
}
