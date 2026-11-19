//! Nesting of rings whose vertices all lie on another ring, decided by
//! the sector an edge enters, and the meetings of one-point polylines.

use std::cmp::Ordering;

use super::super::meetings::hits;
use super::super::ring::{contains, orientation};
use super::super::segment::Meeting;
use super::super::{Point, validate};
use super::wkt;

const SQUARE: [Point; 5] = [[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0], [0.0, 0.0]];

/// Verifies: [ISO/IEC 13249-3:2016 §8.2.1] a ring of no vertices encloses
/// no area and has no turning direction.
#[test]
fn empty_ring_has_no_orientation() {
    assert_eq!(orientation(&[]), Ordering::Equal);
}

/// Verifies: [ISO/IEC 13249-3:2016 §8.2.1, §5.1.10] a ring whose vertices
/// all lie on another ring is inside it when an edge leaving a shared
/// vertex enters the other's interior, from a vertex at the middle of an
/// edge too, whichever way the outer ring turns.
#[test]
fn ring_on_the_boundary_inside() {
    let triangle: [Point; 4] = [[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 0.0]];
    assert!(contains(&SQUARE, &triangle));
    let from_the_middle: [Point; 4] = [[2.0, 0.0], [4.0, 0.0], [4.0, 4.0], [2.0, 0.0]];
    assert!(contains(&SQUARE, &from_the_middle));
    let clockwise: Vec<Point> = SQUARE.iter().rev().copied().collect();
    assert!(contains(&clockwise, &triangle));
}

/// Verifies: [ISO/IEC 13249-3:2016 §8.2.1, §5.1.10] a ring on another's
/// boundary is outside it when its edge leaves a reflex vertex into the
/// notch, and a ring equal to the other is not inside it, no edge
/// leaving the boundary.
#[test]
fn ring_on_the_boundary_outside() {
    let notched: [Point; 6] = [[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [2.0, 1.0], [0.0, 4.0], [0.0, 0.0]];
    let notch: [Point; 4] = [[0.0, 4.0], [2.0, 1.0], [4.0, 4.0], [0.0, 4.0]];
    assert!(!contains(&notched, &notch));
    let under: [Point; 4] = [[2.0, 1.0], [4.0, 4.0], [4.0, 0.0], [2.0, 1.0]];
    assert!(contains(&notched, &under));
    let across: [Point; 4] = [[2.0, 1.0], [0.0, 0.0], [0.0, 4.0], [2.0, 1.0]];
    assert!(contains(&notched, &across));
    assert!(!contains(&SQUARE, &SQUARE));
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.9] a one-point polyline touches
/// a segment through it and a one-point polyline at its position, and
/// meets nothing apart from it.
#[test]
fn one_point_pieces_meet() {
    let lines: Vec<Vec<Point>> = vec![vec![[0.0, 0.0], [2.0, 0.0]], vec![[1.0, 0.0]], vec![[1.0, 0.0]]];
    let found: Vec<Meeting> = hits(&lines).into_iter().map(|hit| hit.meeting).collect();
    assert_eq!(found, vec![Meeting::Touch([1.0_f64, 0.0_f64]); 3]);
    let apart: Vec<Vec<Point>> = vec![vec![[0.0, 0.0], [2.0, 2.0]], vec![[1.5, 0.5]], vec![[0.5, 1.5]]];
    assert!(hits(&apart).is_empty());
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.10] a polygon whose interior ring
/// is one repeated point is not well formed, whether that point lies on
/// the exterior ring, inside it, or where a like ring lies.
#[test]
fn polygon_with_a_point_ring() {
    for source in [
        "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0), (0 0, 0 0, 0 0, 0 0))",
        "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0), (2 0, 2 0, 2 0, 2 0))",
        "POLYGON ((1 1, 1 1, 1 1, 1 1), (1 1, 1 1, 1 1, 1 1))",
    ] {
        assert!(validate(&wkt(source)).unwrap().is_some(), "{source}");
    }
}
