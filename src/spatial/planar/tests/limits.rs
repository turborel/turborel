//! Empty values, degenerate inputs and the collections that carry a
//! malformed part, through validity, simplicity, boundaries, measures,
//! buffers and distance.

use super::super::{
    Dimension, Geometry, Reason, area, boundary, buffer, distance, intersects, is_simple, length, multipolygon_from_rings, perimeter,
    point_on_surface, validate, within,
};
use super::{text, wkt};

fn reason(source: &str) -> Option<Reason> {
    validate(&wkt(source)).unwrap().map(|found| found.reason)
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.10] each reason a value is not
/// well formed has its words.
#[test]
fn reasons_in_words() {
    let all = [
        (Reason::TooFewPoints, "too few points"),
        (Reason::RingNotClosed, "ring not closed"),
        (Reason::RingSelfIntersection, "ring self-intersection"),
        (Reason::RingsCross, "rings cross"),
        (Reason::RingsTouchTwice, "rings meet in more than one point"),
        (Reason::HoleOutsideShell, "interior ring outside the exterior ring"),
        (Reason::NestedHoles, "interior ring inside another"),
        (Reason::DisconnectedInterior, "interior is not connected"),
        (Reason::OverlappingPolygons, "polygon interiors intersect"),
        (Reason::SharedBoundary, "polygon boundaries share a curve"),
    ];
    for (found, words) in all {
        assert_eq!(found.text(), words);
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.10, §9.4, §10.5, §10.6] a
/// multilinestring, multipolygon or collection with a malformed part is
/// not well formed for that part's reason.
#[test]
fn collections_with_a_malformed_part() {
    assert_eq!(reason("MULTILINESTRING ((0 0, 1 1), (0 0))"), Some(Reason::TooFewPoints));
    assert_eq!(reason("MULTIPOLYGON (((0 0, 2 0, 0 0)))"), Some(Reason::TooFewPoints));
    assert_eq!(reason("GEOMETRYCOLLECTION (POINT (1 1), POLYGON ((0 0, 2 0, 0 0)))"), Some(Reason::TooFewPoints));
    assert_eq!(reason("GEOMETRYCOLLECTION (POINT (1 1), LINESTRING (0 0, 1 1))"), None);
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.9] a multilinestring one of whose
/// lines crosses itself is not simple, nor a collection holding one;
/// collinear lines apart are.
#[test]
fn simplicity_of_collections() {
    assert!(!is_simple(&wkt("MULTILINESTRING ((0 0, 2 2, 2 0, 0 2))")).unwrap());
    assert!(!is_simple(&wkt("GEOMETRYCOLLECTION (POINT (5 5), LINESTRING (0 0, 2 2, 2 0, 0 2))")).unwrap());
    assert!(is_simple(&wkt("MULTILINESTRING ((0 0, 1 0), (2 0, 3 0))")).unwrap());
    assert!(!intersects(&wkt("LINESTRING (0 0, 1 0)"), &wkt("LINESTRING (2 0, 3 0)")).unwrap().unwrap());
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.14, §9.1.2, §8.1.3, §8.1.5,
/// §5.1.13] a multipolygon's boundary is its rings; empty values have an
/// empty boundary and no length or perimeter; a collection is of its
/// highest part's dimension; a point on a holed surface is inside it.
#[test]
fn empty_values_and_holed_surfaces() {
    let rings = boundary(&wkt("MULTIPOLYGON (((0 0, 1 0, 1 1, 0 0)), ((5 5, 6 5, 6 6, 5 5)))")).unwrap();
    assert_eq!(text(&rings), "MULTILINESTRING ((0 0, 1 0, 1 1, 0 0), (5 5, 6 5, 6 6, 5 5))");
    assert_eq!(boundary(&wkt("MULTIPOLYGON EMPTY")).unwrap(), Geometry::Empty);
    assert_eq!(boundary(&wkt("GEOMETRYCOLLECTION EMPTY")).unwrap(), Geometry::Empty);
    assert_eq!(length(&wkt("GEOMETRYCOLLECTION EMPTY")), None);
    assert_eq!(perimeter(&wkt("MULTIPOLYGON EMPTY")), None);
    let mixed = wkt("GEOMETRYCOLLECTION (POINT (1 1), POLYGON ((0 0, 1 0, 1 1, 0 0)))");
    assert_eq!(mixed.dimension(), Dimension::Surface);
    let holed = wkt("POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (1 1, 9 1, 9 9, 1 9, 1 1))");
    let on = point_on_surface(&holed).unwrap();
    assert!(within(&Geometry::Point(on), &holed).unwrap().unwrap(), "{on:?}");
    assert_eq!(multipolygon_from_rings(&[]).unwrap(), Geometry::MultiPolygon(Vec::new()));
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.17, §5.1.15] a line string of one
/// repeated point buffers to a disc, as a ring of one repeated point
/// does; parallel lines of many segments are their gap apart.
#[test]
fn degenerate_buffers_and_long_lines() {
    for source in ["LINESTRING (1 1, 1 1)", "POLYGON ((1 1, 1 1, 1 1, 1 1))"] {
        let disc = area(&buffer(&wkt(source), 1.0_f64).unwrap()).unwrap();
        assert!(disc > 3.0_f64 && disc <= std::f64::consts::PI, "{source}: {disc}");
    }
    let low = wkt("LINESTRING (0 0, 1 0, 2 0, 3 0, 4 0, 5 0, 6 0, 7 0, 8 0)");
    let high = wkt("LINESTRING (0 2, 1 2, 2 2, 3 2, 4 2, 5 2, 6 2, 7 2, 8 2)");
    assert_eq!(distance(&low, &high), Some(2.0_f64));
    assert_eq!(distance(&high, &low), Some(2.0_f64));
}
