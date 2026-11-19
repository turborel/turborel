//! The 3D engine against values whose answers are known by hand.

use super::distance::distance;
use super::measure::{centroid, point_on, slope, surface_area, volume};
use super::ops::{SetOp, equals, intersects, set_op};
use super::shape::{boundary, patches_simple, surface_is_closed};
use crate::geometry::model::{Body, Geom, Surface};
use crate::spatial::formats::wkt::{read_geom, write_geom};

mod boundaries;
mod kinds;
mod mixed;
mod tins;

/// The faces of the box from `low` to `high`, facing out.
fn box_faces(low: [f64; 3], high: [f64; 3]) -> String {
    let [x0, y0, z0] = low;
    let [x1, y1, z1] = high;
    let faces = [
        [[x0, y0, z0], [x0, y1, z0], [x1, y1, z0], [x1, y0, z0]],
        [[x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]],
        [[x0, y0, z0], [x1, y0, z0], [x1, y0, z1], [x0, y0, z1]],
        [[x0, y1, z0], [x0, y1, z1], [x1, y1, z1], [x1, y1, z0]],
        [[x0, y0, z0], [x0, y0, z1], [x0, y1, z1], [x0, y1, z0]],
        [[x1, y0, z0], [x1, y1, z0], [x1, y1, z1], [x1, y0, z1]],
    ];
    let ring = |f: &[[f64; 3]; 4]| {
        let points: Vec<String> = f.iter().chain(std::iter::once(&f[0])).map(|p| format!("{} {} {}", p[0], p[1], p[2])).collect();
        format!("POLYGON (({}))", points.join(", "))
    };
    let all: Vec<String> = faces.iter().map(ring).collect();
    format!("POLYHEDRALSURFACE Z (PATCHES ({}))", all.join(", "))
}

fn cube(low: [f64; 3], high: [f64; 3]) -> Geom {
    read_geom(&format!("BREPSOLID Z ({})", box_faces(low, high)), 0).unwrap()
}

fn wkt(text: &str) -> Geom {
    read_geom(text, 0).unwrap()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §9.1.2 to §9.1.5] a box's
/// volume, surface area and centroid, and a point inside it.
#[test]
fn a_cube_measures() {
    let g = cube([0.0_f64; 3], [1.0_f64, 2.0_f64, 3.0_f64]);
    assert!(close(volume(&g).unwrap(), 6.0_f64));
    assert!(close(surface_area(&g).unwrap(), 22.0_f64));
    let c = centroid(&g).unwrap().unwrap();
    assert!(close(c[0], 0.5_f64) && close(c[1], 1.0_f64) && close(c[2], 1.5_f64));
    let p = point_on(&g).unwrap().unwrap();
    assert!(close(p[0], 0.5_f64) && close(p[2], 1.5_f64));
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.31, §5.1.34,
/// §5.1.36, §5.1.38, §5.1.44, §5.1.49] two overlapping cubes' union,
/// intersection, difference and symmetric difference measure as their
/// overlap says; they intersect and are not equal.
#[test]
fn overlapping_cubes_combine() {
    let (a, b) = (cube([0.0_f64; 3], [2.0_f64; 3]), cube([1.0_f64; 3], [3.0_f64; 3]));
    let measure = |op| volume(&set_op(op, &a, &b).unwrap()).unwrap();
    assert!(close(measure(SetOp::Union), 15.0_f64));
    assert!(close(measure(SetOp::Intersection), 1.0_f64));
    assert!(close(measure(SetOp::Difference), 7.0_f64));
    assert!(close(measure(SetOp::SymDifference), 14.0_f64));
    assert!(intersects(&a, &b).unwrap());
    assert!(!equals(&a, &b).unwrap());
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.34, §5.1.31]
/// two cubes sharing a face unite into one box of six merged faces, and
/// meet in that face.
#[test]
fn adjacent_cubes_merge_into_a_box() {
    let (a, b) = (cube([0.0_f64; 3], [1.0_f64; 3]), cube([1.0_f64, 0.0_f64, 0.0_f64], [2.0_f64, 1.0_f64, 1.0_f64]));
    let union = set_op(SetOp::Union, &a, &b).unwrap();
    let Body::Solid(shells) = &union.body else { panic!("{}", write_geom(&union).unwrap()) };
    let Some(Surface::PolyhdrlSurface(patches)) = shells.first() else { panic!("no shell") };
    assert_eq!(patches.len(), 6, "{}", write_geom(&union).unwrap());
    assert!(equals(&union, &cube([0.0_f64; 3], [2.0_f64, 1.0_f64, 1.0_f64])).unwrap());
    let touch = set_op(SetOp::Intersection, &a, &b).unwrap();
    assert_eq!(write_geom(&touch).unwrap(), "POLYGON Z ((1 0 0, 1 1 0, 1 1 1, 1 0 1, 1 0 0))");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.36, §9.2.1] a cube
/// taken out of a larger one leaves a solid of two shells, the inner one
/// a cavity.
#[test]
fn a_cavity_stays_a_cavity() {
    let hollow = set_op(SetOp::Difference, &cube([0.0_f64; 3], [3.0_f64; 3]), &cube([1.0_f64; 3], [2.0_f64; 3])).unwrap();
    assert!(close(volume(&hollow).unwrap(), 26.0_f64));
    let Body::Solid(shells) = &hollow.body else { panic!("not a solid") };
    assert_eq!(shells.len(), 2);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.31, §5.1.36,
/// §5.1.49] lines, points and planes meet a solid in the parts within it.
#[test]
fn lower_dimensions_meet_solids() {
    let solid = cube([0.0_f64; 3], [2.0_f64; 3]);
    let line = wkt("LINESTRING Z (-1 1 1, 3 1 1)");
    assert_eq!(write_geom(&set_op(SetOp::Intersection, &line, &solid).unwrap()).unwrap(), "LINESTRING Z (0 1 1, 2 1 1)");
    let outside = set_op(SetOp::Difference, &line, &solid).unwrap();
    assert_eq!(write_geom(&outside).unwrap(), "MULTILINESTRING Z ((-1 1 1, 0 1 1), (2 1 1, 3 1 1))");
    let point = wkt("POINT Z (1 1 2)");
    assert!(intersects(&point, &solid).unwrap());
    assert!(!intersects(&wkt("POINT Z (1 1 2.5)"), &solid).unwrap());
    let plane = wkt("POLYGON Z ((-1 -1 1, 3 -1 1, 3 3 1, -1 3 1, -1 -1 1))");
    let cut = set_op(SetOp::Intersection, &plane, &solid).unwrap();
    assert_eq!(write_geom(&cut).unwrap(), "POLYGON Z ((0 0 1, 2 0 1, 2 2 1, 0 2 1, 0 0 1))");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.31] two
/// crossing polygons in space meet in a segment.
#[test]
fn crossing_surfaces_meet_in_a_segment() {
    let one = wkt("POLYGON Z ((0 0 0, 2 0 0, 2 2 0, 0 2 0, 0 0 0))");
    let other = wkt("POLYGON Z ((1 -1 -1, 1 3 -1, 1 3 1, 1 -1 1, 1 -1 -1))");
    assert_eq!(write_geom(&set_op(SetOp::Intersection, &one, &other).unwrap()).unwrap(), "LINESTRING Z (1 0 0, 1 2 0)");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.42] the least distance in
/// space between solids, a point and a line.
#[test]
fn distances_in_space() {
    let (a, b) = (cube([0.0_f64; 3], [1.0_f64; 3]), cube([2.0_f64, 0.0_f64, 0.0_f64], [3.0_f64, 1.0_f64, 1.0_f64]));
    assert!(close(distance(&a, &b).unwrap().unwrap(), 1.0_f64));
    assert!(close(distance(&a, &wkt("POINT Z (0.5 0.5 0.5)")).unwrap().unwrap(), 0.0_f64));
    assert!(close(distance(&wkt("POINT Z (0 0 5)"), &wkt("LINESTRING Z (0 0 0, 3 0 4)")).unwrap().unwrap(), 3.0_f64));
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.1.11, §8.1.13,
/// §5.1.9, §5.1.18] a box's faces are a closed, simple shell; an open
/// surface's boundary is its rim, and crossing patches are not simple.
#[test]
fn shells_close_and_are_simple() {
    let shell = wkt(&box_faces([0.0_f64; 3], [1.0_f64; 3]));
    assert!(surface_is_closed(&shell));
    let Body::Surface(s) = &shell.body else { panic!("not a surface") };
    assert!(patches_simple(&[s], shell.dims).unwrap(), "a closed shell is simple");
    let open = wkt("POLYHEDRALSURFACE Z (PATCHES (POLYGON ((0 0 0, 1 0 0, 1 1 0, 0 0 0)), POLYGON ((0 0 0, 1 1 0, 0 1 0, 0 0 0))))");
    assert!(!surface_is_closed(&open));
    let Some(edges) = boundary(&open).unwrap() else { panic!("no boundary") };
    assert_eq!(write_geom(&edges).unwrap(), "LINESTRING Z (0 0 0, 1 0 0, 1 1 0, 0 1 0, 0 0 0)");
    let crossing =
        wkt("POLYHEDRALSURFACE Z (PATCHES (POLYGON ((0 0 0, 2 0 0, 2 2 0, 0 0 0)), POLYGON ((1 -1 -1, 1 3 -1, 1 1 3, 1 -1 -1))))");
    let Body::Surface(crossed) = &crossing.body else { panic!("not a surface") };
    assert!(!patches_simple(&[crossed], crossing.dims).unwrap());
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.4.4] a triangle's slope is its rise
/// over its run, none of a vertical one.
#[test]
fn slopes() {
    assert_eq!(slope([[0.0_f64; 3], [1.0_f64, 0.0_f64, 1.0_f64], [0.0_f64, 1.0_f64, 0.0_f64]]), Some(1.0_f64));
    assert_eq!(slope([[0.0_f64; 3], [1.0_f64, 0.0_f64, 0.0_f64], [0.0_f64, 0.0_f64, 1.0_f64]]), None);
}

/// Two triangles making the unit square, z of x + y, and their corners
/// as random points.
const SQUARE: &str = "TIN Z (PATCHES ((0 0 0, 2 0 2, 2 2 4), (0 0 0, 2 2 4, 0 2 2)) ELEMENTS (POINTS ID 1 TAG \"p\" MULTIPOINT Z ((0 0 0), (2 0 2), (2 2 4), (0 2 2))))";

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.6.4, §8.6.6] a TIN
/// retriangulates under a maximum side length, and its clip keeps the
/// area within a polygon, lifted onto its planes, the polygon a boundary.
#[test]
fn a_tin_retriangulates_and_clips() {
    let tin = wkt(SQUARE);
    let again = super::tin::with_max_side(&tin, 5.0_f64, true).unwrap();
    let Body::Surface(Surface::Tin(t)) = &again.body else { panic!("not a TIN") };
    assert_eq!((t.triangles.len(), t.max_side_length), (2, Some(5.0_f64)));
    let clipped =
        super::tin::clip(&tin, &[vec![[0.0_f64, 0.0_f64], [1.0_f64, 0.0_f64], [1.0_f64, 1.0_f64], [0.0_f64, 1.0_f64], [0.0_f64, 0.0_f64]]])
            .unwrap();
    let Body::Surface(Surface::Tin(cut)) = &clipped.body else { panic!("not a TIN") };
    let area: f64 = cut
        .triangles
        .iter()
        .map(|r| {
            let p: Vec<&[f64]> = r.chunks(3).collect();
            ((p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[1][1] - p[0][1]) * (p[2][0] - p[0][0])) / 2.0_f64
        })
        .sum();
    assert!(close(area, 1.0_f64), "{}", write_geom(&clipped).unwrap());
    assert!(cut.triangles.iter().flat_map(|r| r.chunks(3)).all(|p| close(p[2], p[0] + p[1])), "lifted onto the planes");
    assert!(cut.elements.iter().any(|e| e.element_type == "boundary"));
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.6.5] a TIN's points,
/// triangles and elements as rows, and the TIN built back from them.
#[test]
fn a_tin_becomes_rows_and_back() {
    let rows = super::table::table(&wkt(SQUARE)).unwrap();
    let items: Vec<(&str, i64)> = rows.iter().map(|r| (r.item.as_str(), r.order)).collect();
    assert_eq!(items, [("point", 1), ("point", 2), ("point", 3), ("point", 4), ("triangle", 1), ("triangle", 2), ("random points", 1)]);
    assert_eq!(rows[1].point, Some((0.0_f64, 2.0_f64, Some(2.0_f64))));
    assert_eq!((rows[4].triangle, rows[4].visibility), (Some([1, 2, 4]), Some(1)));
    assert_eq!(rows[6].element, Some(vec![1, 3, 4, 2]));
    let back = super::table::from_rows(&rows, 9.0_f64).unwrap();
    let Body::Surface(Surface::Tin(t)) = &back.body else { panic!("not a TIN") };
    assert_eq!((t.triangles.len(), t.elements.len(), t.max_side_length), (2, 1, Some(9.0_f64)));
    assert!(super::table::from_rows(&rows[..2], 1.0_f64).is_err(), "three points at least");
}
