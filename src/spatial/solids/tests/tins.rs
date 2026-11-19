//! A TIN's checks, and its rows with holes, voids and stop lines.

use super::super::table::{Row, from_rows, table};
use super::super::tin::{tin_of, with_max_side};
use super::{SQUARE, wkt};
use crate::geometry::model::{Body, Surface};

/// The SQLSTATE of `r`'s error.
fn state<T: std::fmt::Debug>(r: crate::error::Result<T>) -> String {
    r.unwrap_err().sqlstate().as_str().to_owned()
}

/// The square of [`SQUARE`]'s two triangles, the first a hole, the second
/// a void, crossed by a stop line, and bounded by a polygon with a hole.
const MARKED: &str = "TIN Z (PATCHES ((0 0 0, 2 0 2, 2 2 4), (0 0 0, 2 2 4, 0 2 2)) ELEMENTS (\
     HOLE ID 1 TAG \"h\" POLYGON Z ((0 0 0, 2 0 2, 2 2 4, 0 0 0)), \
     VOID ID 2 TAG \"v\" POLYGON Z ((0 0 0, 2 2 4, 0 2 2, 0 0 0)), \
     STOPLINE ID 3 TAG \"s\" LINESTRING Z (0 0 0, 2 2 4), \
     BOUNDARY ID 4 TAG \"b\" POLYGON Z ((0 0 0, 2 0 2, 2 2 4, 0 2 2, 0 0 0), (0 0 0, 2 2 4, 0 2 2, 0 0 0))))";

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.6, §8.6.4
/// Description 4b] a value that is no TIN is refused with 2FF70, and a
/// maximum side length not above 0 with 2FF02; without retriangulation the
/// length is only recorded.
#[test]
fn tin_checks() {
    assert_eq!(state(tin_of(&wkt("POINT Z (0 0 0)"))), "2FF70");
    assert_eq!(state(with_max_side(&wkt(SQUARE), 0.0_f64, true)), "2FF02");
    assert_eq!(state(with_max_side(&wkt(SQUARE), f64::NAN, false)), "2FF02");
    let kept = with_max_side(&wkt(SQUARE), 1.0_f64, false).unwrap();
    let Body::Surface(Surface::Tin(t)) = &kept.body else { panic!("not a TIN") };
    assert_eq!((t.triangles.len(), t.max_side_length), (2, Some(1.0_f64)));
    let bare = wkt("TIN Z (PATCHES ((0 0 0, 2 0 0, 0 2 0)))");
    let again = with_max_side(&bare, 10.0_f64, true).unwrap();
    let Body::Surface(Surface::Tin(one)) = &again.body else { panic!("not a TIN") };
    assert_eq!(one.triangles.len(), 1);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.6.5 D3f i 7,
/// Description 4b, 5, 5a i] a triangle within a hole is of visibility
/// -1, one within a void 0; the rows come back to the same elements, and
/// rows naming no point, no element type, too few points or a bad length
/// are refused.
#[test]
fn tin_rows_mark_holes_and_voids() {
    let rows = table(&wkt(MARKED)).unwrap();
    let mut seen: Vec<Option<i64>> = rows.iter().filter(|r| r.item == "triangle").map(|r| r.visibility).collect();
    seen.sort_unstable();
    assert_eq!(seen, [Some(-1_i64), Some(0_i64)]);
    assert!(rows.iter().any(|r| r.item.ends_with(" hole")), "the boundary's hole is a row");
    let back = from_rows(&rows, 9.0_f64).unwrap();
    let Body::Surface(Surface::Tin(t)) = &back.body else { panic!("not a TIN") };
    assert_eq!((t.triangles.len(), t.elements.len()), (2, 4));
    assert_eq!(state(from_rows(&rows, -1.0_f64)), "2FF02");
    assert_eq!(state(from_rows(&rows[..2], 9.0_f64)), "2FF71");
    let mut unknown = rows.clone();
    unknown.push(Row { item: "wall".to_owned(), order: 9, element: Some(vec![1, 2]), ..Row::default() });
    assert_eq!(state(from_rows(&unknown, 9.0_f64)), "2FF02");
    let mut orphan = rows.clone();
    orphan.push(Row { item: "void hole".to_owned(), order: 9, id: Some(77), element: Some(vec![1, 2, 3, 1]), ..Row::default() });
    assert_eq!(state(from_rows(&orphan, 9.0_f64)), "2FF02");
    let mut missing = rows;
    missing.push(Row { item: "triangle".to_owned(), order: 9, triangle: Some([1, 2, 99]), ..Row::default() });
    assert_eq!(state(from_rows(&missing, 9.0_f64)), "2FF02");
}
