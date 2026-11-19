//! The 3D engine on collections of every kind, curved and compound
//! values, and patches of no area.

use super::super::ops::intersects;
use super::super::shape::boundary;
use super::{cube, wkt};
use crate::geometry::model::{Body, Collection, Curve};
use crate::spatial::formats::wkt::write_geom;

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.44] each kind
/// of collection, a compound curve, a polygon of no area and a collection
/// holding a solid meet a cube where a part lies in it, and not where none
/// does.
#[test]
fn every_kind_meets_a_cube() {
    let solid = cube([0.0_f64; 3], [2.0_f64; 3]);
    let meeting = [
        "MULTIPOINT Z ((5 5 5), (1 1 1))",
        "MULTILINESTRING Z ((5 5 5, 6 6 6), (1 1 1, 1 1 3))",
        "MULTICURVE Z ((5 5 5, 6 6 6), (1 1 1, 3 1 1))",
        "COMPOUNDCURVE Z ((5 5 5, 1 1 1), (1 1 1, 1 5 1))",
        "MULTIPOLYGON Z (((1 1 1, 3 1 1, 3 3 1, 1 1 1)))",
        "MULTISURFACE Z (((1 1 1, 3 1 1, 3 3 1, 1 1 1)))",
        "POLYGON Z ((1 1 1, 3 1 1, 5 1 1, 1 1 1))",
        "GEOMETRYCOLLECTION Z (POINT Z (9 9 9), GEOMETRYCOLLECTION Z (LINESTRING Z (1 1 1, 1 1 5)))",
    ];
    for text in meeting {
        assert!(intersects(&wkt(text), &solid).unwrap(), "{text}");
        assert!(intersects(&solid, &wkt(text)).unwrap(), "{text}");
    }
    let held = wkt(&format!("GEOMETRYCOLLECTION Z ({})", write_geom(&cube([5.0_f64; 3], [6.0_f64; 3])).unwrap()));
    assert!(!intersects(&held, &solid).unwrap());
    assert!(intersects(&held, &wkt("POINT Z (5.5 5.5 5.5)")).unwrap());
    assert!(!intersects(&wkt("POLYGON Z ((5 5 5, 6 5 5, 7 5 5, 5 5 5))"), &solid).unwrap());
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.18] the boundary
/// of a TIN, a compound surface and a multisurface of curve polygons is
/// the edges one patch uses, in space.
#[test]
fn boundaries_of_patch_surfaces() {
    let tin = boundary(&wkt("TIN Z (PATCHES ((0 0 0, 2 0 0, 2 2 0), (0 0 0, 2 2 0, 0 2 0)))")).unwrap().unwrap();
    let Body::Curve(Curve::LineString(rim)) = &tin.body else { panic!("{}", write_geom(&tin).unwrap()) };
    assert_eq!(rim.len(), 15, "{}", write_geom(&tin).unwrap());
    let compound = wkt("COMPOUNDSURFACE Z (POLYGON Z ((0 0 0, 1 0 0, 1 1 0, 0 0 0)), POLYGON Z ((0 0 0, 1 1 0, 0 1 0, 0 0 0)))");
    let rim2 = boundary(&compound).unwrap().unwrap();
    let Body::Curve(Curve::LineString(points)) = &rim2.body else { panic!("{}", write_geom(&rim2).unwrap()) };
    assert_eq!(points.len(), 15);
    let curved =
        wkt("MULTISURFACE Z (CURVEPOLYGON Z (COMPOUNDCURVE Z ((0 0 0, 4 0 0), (4 0 0, 4 4 0, 0 0 0))), ((9 9 9, 10 9 9, 10 10 9, 9 9 9)))");
    let both = boundary(&curved).unwrap().unwrap();
    let Body::Collection(Collection::MultiLineString(rings)) = &both.body else { panic!("{}", write_geom(&both).unwrap()) };
    assert_eq!(rings.len(), 2);
}
