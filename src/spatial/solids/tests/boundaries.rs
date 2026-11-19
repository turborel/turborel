//! The 3D boundary of each kind of value, and the closedness of surfaces.

use super::super::shape::{boundary, surface_is_closed};
use super::{box_faces, cube, wkt};
use crate::geometry::model::{Body, Collection, Curve, Surface};

/// The boundary of the value `text` writes, which is not null.
fn bound(text: &str) -> Body {
    boundary(&wkt(text)).unwrap().unwrap().body
}

/// Whether `body` is the empty collection.
fn is_none(body: &Body) -> bool {
    matches!(body, Body::Collection(Collection::Geometry(_))) && count(body) == 0
}

/// How many parts a collection `body` has; 1 of any other.
fn count(body: &Body) -> usize {
    match body {
        Body::Collection(Collection::Geometry(p)) => p.len(),
        Body::Collection(Collection::MultiPoint(p) | Collection::MultiLineString(p)) => p.len(),
        Body::Collection(Collection::MultiCurve(p)) => p.len(),
        Body::Collection(Collection::MultiSurface(p)) => p.len(),
        Body::Collection(Collection::MultiPolygon(p)) => p.len(),
        Body::Point(_) | Body::Curve(_) | Body::Surface(_) | Body::Solid(_) => 1,
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.18] a point has
/// no boundary, an empty value a null one, an open curve its two ends, a
/// closed curve none, and curves meeting end to end the ends an odd number
/// of them have.
#[test]
fn curve_boundaries_are_odd_ends() {
    assert!(is_none(&bound("POINT Z (1 2 3)")));
    assert!(is_none(&bound("MULTIPOINT Z ((1 2 3), (4 5 6))")));
    assert!(boundary(&wkt("POINT Z EMPTY")).unwrap().is_none());
    let Body::Collection(Collection::MultiPoint(ends)) = bound("LINESTRING Z (0 0 0, 1 0 0, 1 1 1)") else { panic!("no ends") };
    assert_eq!(ends, [vec![0.0_f64, 0.0_f64, 0.0_f64], vec![1.0_f64, 1.0_f64, 1.0_f64]]);
    assert!(is_none(&bound("LINESTRING Z (0 0 0, 1 0 0, 1 1 0, 0 0 0)")));
    let Body::Collection(Collection::MultiPoint(joined)) = bound("MULTILINESTRING Z ((0 0 0, 1 0 0), (1 0 0, 2 0 0))") else {
        panic!("no ends")
    };
    assert_eq!(joined, [vec![0.0_f64, 0.0_f64, 0.0_f64], vec![2.0_f64, 0.0_f64, 0.0_f64]]);
    let Body::Collection(Collection::MultiPoint(curved)) = bound("MULTICURVE Z ((0 0 0, 1 0 0), (1 0 0, 1 1 0))") else {
        panic!("no ends")
    };
    assert_eq!(curved.len(), 2);
    let Body::Collection(Collection::MultiPoint(measured)) = bound("LINESTRING ZM (0 0 0 5, 1 0 0 6)") else { panic!("no ends") };
    assert_eq!(measured, [vec![0.0_f64, 0.0_f64, 0.0_f64, 5.0_f64], vec![1.0_f64, 0.0_f64, 0.0_f64, 6.0_f64]]);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.18] a polygon's
/// boundary is its rings, a curve polygon's its ring curves, and a
/// multipolygon's or multisurface's the edges one patch uses.
#[test]
fn surface_boundaries_are_rings() {
    assert!(matches!(bound("POLYGON Z ((0 0 0, 4 0 0, 4 4 0, 0 0 0))"), Body::Curve(Curve::LineString(_))));
    let holed = bound("POLYGON Z ((0 0 0, 4 0 0, 4 4 0, 0 4 0, 0 0 0), (1 1 0, 1 2 0, 2 2 0, 1 1 0))");
    assert!(matches!(holed, Body::Collection(Collection::MultiLineString(_))) && count(&holed) == 2);
    let arc = bound("CURVEPOLYGON Z (CIRCULARSTRING Z (0 0 0, 1 1 0, 2 0 0, 1 -1 0, 0 0 0))");
    assert!(matches!(arc, Body::Curve(Curve::CircularString(_))));
    let arcs = bound("CURVEPOLYGON Z (CIRCULARSTRING Z (0 0 0, 4 4 0, 8 0 0, 4 -4 0, 0 0 0), (3 0 0, 4 1 0, 5 0 0, 3 0 0))");
    assert!(matches!(arcs, Body::Collection(Collection::MultiCurve(_))) && count(&arcs) == 2);
    let two = bound("MULTIPOLYGON Z (((0 0 0, 1 0 0, 1 1 0, 0 0 0)), ((5 5 5, 6 5 5, 6 6 5, 5 5 5)))");
    assert!(matches!(two, Body::Collection(Collection::MultiLineString(_))) && count(&two) == 2);
    let one = bound("MULTISURFACE Z (((0 0 0, 1 0 0, 1 1 0, 0 1 0, 0 0 0)))");
    let Body::Curve(Curve::LineString(points)) = one else { panic!("no ring") };
    assert_eq!(points.len(), 15);
    let flat = bound("MULTIPOLYGON (((0 0, 1 0, 1 1, 0 0)))");
    let Body::Curve(Curve::LineString(plan)) = flat else { panic!("no ring") };
    assert_eq!(plan.len(), 8);
    assert!(matches!(bound("TRIANGLE Z (0 0 0, 1 0 0, 0 1 1)"), Body::Curve(Curve::LineString(_))));
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.18, §8.1.11] a
/// closed shell has no boundary and is closed, a solid's boundary is its
/// shells, and a collection's is its parts' boundaries.
#[test]
fn shells_and_collections_bound() {
    assert!(is_none(&bound(&box_faces([0.0_f64; 3], [1.0_f64; 3]))));
    let solid = boundary(&cube([0.0_f64; 3], [1.0_f64; 3])).unwrap().unwrap();
    assert!(matches!(&solid.body, Body::Surface(Surface::PolyhdrlSurface(_))));
    assert!(surface_is_closed(&solid));
    let hollow = wkt(&format!("BREPSOLID Z ({}, {})", box_faces([0.0_f64; 3], [3.0_f64; 3]), box_faces([1.0_f64; 3], [2.0_f64; 3])));
    let shells = boundary(&hollow).unwrap().unwrap().body;
    assert!(matches!(shells, Body::Collection(Collection::MultiSurface(_))) && count(&shells) == 2);
    assert!(!surface_is_closed(&hollow), "a solid is no surface");
    let mixed =
        bound("GEOMETRYCOLLECTION Z (POINT Z (9 9 9), LINESTRING Z (0 0 0, 1 0 0), MULTIPOLYGON Z (((0 0 0, 1 0 0, 1 1 0, 0 0 0))))");
    let Body::Collection(Collection::Geometry(parts)) = mixed else { panic!("no collection") };
    assert_eq!(parts.len(), 3);
    assert!(is_none(&parts[0]));
    assert!(matches!(&parts[1], Body::Collection(Collection::MultiPoint(_))) && count(&parts[1]) == 2);
    assert!(matches!(&parts[2], Body::Curve(Curve::LineString(_))));
    let closed = bound(&format!("GEOMETRYCOLLECTION Z ({})", box_faces([0.0_f64; 3], [1.0_f64; 3])));
    let Body::Collection(Collection::Geometry(inner)) = closed else { panic!("no collection") };
    assert!(inner.len() == 1 && is_none(&inner[0]));
}
