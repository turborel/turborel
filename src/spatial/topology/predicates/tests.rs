//! Tests of the exact planar predicates of topology-geometry.

use super::*;

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.53] a point is
/// on a line anywhere along it, and inside it only off its two ends.
#[test]
fn points_on_lines() {
    let l = [[0.0_f64, 0.0_f64], [4.0_f64, 0.0_f64], [4.0_f64, 4.0_f64]];
    assert!(on_line([2.0_f64, 0.0_f64], &l));
    assert!(on_line([4.0_f64, 4.0_f64], &l));
    assert!(!on_line([2.0_f64, 0.1_f64], &l));
    assert!(inside_line([4.0_f64, 2.0_f64], &l));
    assert!(inside_line([4.0_f64, 0.0_f64], &l));
    assert!(!inside_line([0.0_f64, 0.0_f64], &l));
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.8] a line is
/// simple unless it meets itself, a closed one at its ends allowed.
#[test]
fn simple_lines() {
    assert!(is_simple(&[[0.0_f64, 0.0_f64], [4.0_f64, 0.0_f64], [4.0_f64, 4.0_f64]]));
    assert!(is_simple(&[[0.0_f64, 0.0_f64], [4.0_f64, 0.0_f64], [4.0_f64, 4.0_f64], [0.0_f64, 0.0_f64]]));
    assert!(!is_simple(&[[0.0_f64, 0.0_f64], [4.0_f64, 4.0_f64], [4.0_f64, 0.0_f64], [0.0_f64, 4.0_f64]]));
    assert!(!is_simple(&[[0.0_f64, 0.0_f64], [4.0_f64, 0.0_f64], [2.0_f64, 0.0_f64]]));
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.12] lines meet
/// where they cross or touch, but not at a place that is allowed.
#[test]
fn lines_meet() {
    let a = [[0.0_f64, 0.0_f64], [4.0_f64, 4.0_f64]];
    let b = [[0.0_f64, 4.0_f64], [4.0_f64, 0.0_f64]];
    assert_eq!(meet_except(&a, &b, &[]), Some([2.0_f64, 2.0_f64]));
    let c = [[4.0_f64, 4.0_f64], [8.0_f64, 4.0_f64]];
    assert_eq!(meet_except(&a, &c, &[[4.0_f64, 4.0_f64]]), None);
    assert_eq!(meet_except(&a, &c, &[]), Some([4.0_f64, 4.0_f64]));
    let d = [[5.0_f64, 0.0_f64], [9.0_f64, 0.0_f64]];
    assert_eq!(meet_except(&a, &d, &[]), None);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.16] rings turn
/// one way or the other, and a point is inside, on or outside one.
#[test]
fn rings() {
    let ccw = [[0.0_f64, 0.0_f64], [4.0_f64, 0.0_f64], [4.0_f64, 4.0_f64], [0.0_f64, 4.0_f64], [0.0_f64, 0.0_f64]];
    let cw: Vec<Point> = ccw.iter().rev().copied().collect();
    assert!(counterclockwise(&ccw));
    assert!(!counterclockwise(&cw));
    assert_eq!(area(&ccw), 16.0_f64);
    assert_eq!(side([2.0_f64, 2.0_f64], &ccw), Side::Inside);
    assert_eq!(side([4.0_f64, 2.0_f64], &ccw), Side::On);
    assert_eq!(side([5.0_f64, 2.0_f64], &ccw), Side::Outside);
    assert_eq!(side([2.0_f64, 2.0_f64], &cw), Side::Inside);
    assert_eq!(bounds(&ccw), Some([0.0_f64, 0.0_f64, 4.0_f64, 4.0_f64]));
}
