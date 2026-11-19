//! The 3D set operations, relationships and distance on values of every
//! dimension, and solids whose faces cut one another's planes.

use super::super::distance::distance;
use super::super::measure::volume;
use super::super::ops::{SetOp, equals, intersects, set_op};
use super::{box_faces, close, cube, wkt};
use crate::geometry::model::Geom;
use crate::spatial::formats::wkt::write_geom;

/// The corner tetrahedron of legs `size` along the axes, its faces facing
/// inward, so that its shell is turned before use.
fn tetrahedron(size: f64) -> Geom {
    let s = size;
    wkt(&format!(
        "BREPSOLID Z (POLYHEDRALSURFACE Z (PATCHES (POLYGON ((0 0 0, {s} 0 0, 0 {s} 0, 0 0 0)), \
         POLYGON ((0 0 0, 0 0 {s}, {s} 0 0, 0 0 0)), POLYGON ((0 0 0, 0 {s} 0, 0 0 {s}, 0 0 0)), \
         POLYGON (({s} 0 0, 0 0 {s}, 0 {s} 0, {s} 0 0)))))"
    ))
}

/// The box from `low` to `high`, its faces facing inward.
fn inverted(low: [f64; 3], high: [f64; 3]) -> Geom {
    let faces = box_faces(low, high);
    let body = faces.trim_start_matches("POLYHEDRALSURFACE Z (PATCHES (").trim_end_matches("))");
    let turned: Vec<String> = body
        .split("), POLYGON ((")
        .map(|ring| {
            let points: Vec<&str> = ring.trim_start_matches("POLYGON ((").trim_end_matches(')').split(", ").collect();
            format!("POLYGON (({}))", points.into_iter().rev().collect::<Vec<_>>().join(", "))
        })
        .collect();
    wkt(&format!("BREPSOLID Z (POLYHEDRALSURFACE Z (PATCHES ({})))", turned.join(", ")))
}

fn text(g: &Geom) -> String {
    write_geom(g).unwrap()
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.31, §5.1.34,
/// §5.1.36, §9.2.1 D9] a tetrahedron whose slanted face cuts a cube's
/// faces meets it in the cube's corner below that plane, of volume 32/3
/// less three corners of 4/3; the union and difference follow.
#[test]
fn a_slanted_face_cuts_a_cube() {
    let (box_, tetra) = (cube([0.0_f64; 3], [2.0_f64; 3]), tetrahedron(4.0_f64));
    assert!(close(volume(&tetra).unwrap(), 32.0_f64 / 3.0_f64));
    let measure = |op| volume(&set_op(op, &box_, &tetra).unwrap()).unwrap();
    assert!(close(measure(SetOp::Intersection), 20.0_f64 / 3.0_f64));
    assert!(close(measure(SetOp::Union), 12.0_f64));
    assert!(close(measure(SetOp::Difference), 4.0_f64 / 3.0_f64));
    assert!(close(volume(&set_op(SetOp::Difference, &tetra, &box_).unwrap()).unwrap(), 4.0_f64));
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §9.2.1 D9, §5.1.40]
/// a shell given facing inward bounds the same solid as one facing out.
#[test]
fn an_inward_shell_is_turned() {
    let turned = inverted([0.0_f64; 3], [1.0_f64, 2.0_f64, 3.0_f64]);
    assert!(close(volume(&turned).unwrap(), 6.0_f64));
    assert!(equals(&turned, &cube([0.0_f64; 3], [1.0_f64, 2.0_f64, 3.0_f64])).unwrap());
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.31, §5.1.34,
/// §5.1.36, §5.1.44] points meet points, curves and surfaces where they
/// lie on them; repeated points unite into one.
#[test]
fn points_meet_what_they_lie_on() {
    let p = wkt("POINT Z (1 1 1)");
    assert_eq!(text(&set_op(SetOp::Intersection, &p, &p).unwrap()), "POINT Z (1 1 1)");
    let line = wkt("LINESTRING Z (0 0 0, 2 2 2)");
    assert_eq!(text(&set_op(SetOp::Intersection, &p, &line).unwrap()), "POINT Z (1 1 1)");
    assert_eq!(text(&set_op(SetOp::Intersection, &line, &p).unwrap()), "POINT Z (1 1 1)");
    let square = wkt("POLYGON Z ((0 0 1, 2 0 1, 2 2 1, 0 2 1, 0 0 1))");
    assert!(intersects(&p, &square).unwrap());
    assert!(intersects(&square, &p).unwrap());
    assert!(!intersects(&wkt("POINT Z (1 1 2)"), &square).unwrap());
    let both = set_op(SetOp::Union, &wkt("MULTIPOINT Z ((0 0 0), (0 0 0), (3 3 3))"), &wkt("POINT Z (3 3 3)")).unwrap();
    assert!(equals(&both, &wkt("MULTIPOINT Z ((0 0 0), (3 3 3))")).unwrap(), "{}", text(&both));
    let away = set_op(SetOp::Difference, &wkt("MULTIPOINT Z ((1 1 1), (5 5 5))"), &line).unwrap();
    assert_eq!(text(&away), "POINT Z (5 5 5)");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.31, §5.1.44]
/// collinear segments meet in their overlap, crossing ones in a point,
/// a segment within a polygon in itself, and coplanar polygons in their
/// common area.
#[test]
fn curves_and_surfaces_overlap() {
    let (one, other) = (wkt("LINESTRING Z (0 0 0, 2 0 0)"), wkt("LINESTRING Z (1 0 0, 3 0 0)"));
    assert_eq!(text(&set_op(SetOp::Intersection, &one, &other).unwrap()), "LINESTRING Z (1 0 0, 2 0 0)");
    let crossing = wkt("LINESTRING Z (1 -1 0, 1 1 0)");
    assert_eq!(text(&set_op(SetOp::Intersection, &one, &crossing).unwrap()), "POINT Z (1 0 0)");
    let square = wkt("POLYGON Z ((0 0 0, 4 0 0, 4 4 0, 0 4 0, 0 0 0))");
    let inside = wkt("LINESTRING Z (1 1 0, 3 1 0)");
    assert_eq!(text(&set_op(SetOp::Intersection, &inside, &square).unwrap()), "LINESTRING Z (1 1 0, 3 1 0)");
    let shifted = wkt("POLYGON Z ((2 2 0, 6 2 0, 6 6 0, 2 6 0, 2 2 0))");
    let common = set_op(SetOp::Intersection, &square, &shifted).unwrap();
    assert!(equals(&common, &wkt("POLYGON Z ((2 2 0, 4 2 0, 4 4 0, 2 4 0, 2 2 0))")).unwrap(), "{}", text(&common));
    let mixed = set_op(SetOp::Union, &wkt("POINT Z (9 9 9)"), &one).unwrap();
    assert!(equals(&mixed, &wkt("GEOMETRYCOLLECTION Z (POINT Z (9 9 9), LINESTRING Z (0 0 0, 2 0 0))")).unwrap(), "{}", text(&mixed));
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.31] a polygon
/// lying on a cube's face, facing either way, is within the closed solid.
#[test]
fn a_face_on_a_solid() {
    let solid = cube([0.0_f64; 3], [2.0_f64; 3]);
    for face in ["POLYGON Z ((0 0 0, 1 0 0, 1 1 0, 0 1 0, 0 0 0))", "POLYGON Z ((0 0 0, 0 1 0, 1 1 0, 1 0 0, 0 0 0))"] {
        let on = set_op(SetOp::Intersection, &wkt(face), &solid).unwrap();
        assert!(equals(&on, &wkt(face)).unwrap(), "{}", text(&on));
        assert!(intersects(&solid, &wkt(face)).unwrap());
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.42, §5.1.44] the
/// distance to an empty value is null, values that meet are 0 apart, and
/// a point above a triangle is its height from it.
#[test]
fn distances_of_every_kind() {
    let triangle = wkt("TRIANGLE Z (0 0 0, 4 0 0, 0 4 0)");
    assert_eq!(distance(&wkt("POINT Z EMPTY"), &triangle).unwrap(), None);
    assert!(!intersects(&wkt("POINT Z EMPTY"), &triangle).unwrap());
    assert_eq!(distance(&wkt("LINESTRING Z (0 0 0, 2 2 0)"), &wkt("LINESTRING Z (0 2 0, 2 0 0)")).unwrap(), Some(0.0_f64));
    assert!(close(distance(&wkt("POINT Z (1 1 5)"), &triangle).unwrap().unwrap(), 5.0_f64));
    assert!(close(distance(&triangle, &wkt("POINT Z (1 1 -2)")).unwrap().unwrap(), 2.0_f64));
    let apart = distance(&wkt("LINESTRING Z (0 0 3, 0 0 3, 1 0 3)"), &wkt("LINESTRING Z (0 5 0, 0 5 0)")).unwrap().unwrap();
    assert!(close(apart, 34.0_f64.sqrt()));
}
