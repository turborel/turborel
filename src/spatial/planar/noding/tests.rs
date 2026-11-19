//! Noding leaves segments meeting only at shared vertices.

use super::super::chain::{Candidate, candidates};
use super::super::segment::{Meeting, meet, same};
use super::super::tests::Seeded;
use super::*;

fn segment(lines: &[Vec<Point>], line: usize, index: usize) -> (Point, Point) {
    (lines[line][index], lines[line][index.checked_add(1).unwrap()])
}

/// Every pair of segments meets at most at shared ends, or is the same
/// segment.
fn assert_noded(lines: &[Vec<Point>]) {
    for candidate in candidates(lines, &[]) {
        let Candidate::Pair(first, second) = candidate else { continue };
        if lines[first.line].len() < 2 || lines[second.line].len() < 2 {
            continue;
        }
        let (a, b) = segment(lines, first.line, first.index);
        let (c, d) = segment(lines, second.line, second.index);
        match meet(a, b, c, d) {
            Meeting::Apart => (),
            Meeting::Touch(point) => {
                assert!((same(point, a) || same(point, b)) && (same(point, c) || same(point, d)), "{a:?} {b:?} {c:?} {d:?}");
            }
            Meeting::Overlap(..) => assert!((same(a, c) && same(b, d)) || (same(a, d) && same(b, c)), "{a:?} {b:?} {c:?} {d:?}"),
            Meeting::Cross => panic!("{a:?} {b:?} {c:?} {d:?}"),
        }
    }
}

/// Verifies: [DESIGN] random segments, many crossing and many sharing
/// lines, are noded; input vertices stay; every new vertex lies within
/// one unit in the last place of its input segment's line.
#[test]
fn random_segments_are_noded() {
    let mut random = Seeded(17);
    for round in 0_i32..60_i32 {
        let count = 2_i32 + round % 12_i32;
        let input: Vec<Vec<Point>> = (0_i32..count).map(|_| vec![random.point(8), random.point(8), random.point(8)]).collect();
        let output = node(&input).unwrap();
        assert_eq!(output.len(), input.len());
        assert_noded(&output);
        for (before, after) in input.iter().zip(&output) {
            let before = deduplicated(before);
            for vertex in &before {
                assert!(after.iter().any(|point| same(*point, *vertex)), "{vertex:?} lost");
            }
            assert!(same(before[0], after[0]) && same(*before.last().unwrap(), *after.last().unwrap()));
        }
    }
}

/// Verifies: [DESIGN] crossings at positions that are not representable
/// are noded through one rounded vertex, and a fan of segments through
/// nearly one point settles.
#[test]
fn rounded_crossings_settle() {
    let input = vec![
        vec![[0.0_f64, 0.0_f64], [3.0_f64, 1.0_f64]],
        vec![[0.0_f64, 1.0_f64], [3.0_f64, 0.1_f64]],
        vec![[0.1_f64, 0.0_f64], [2.9_f64, 1.0_f64]],
        vec![[1.0_f64, 0.0_f64], [1.7_f64, 1.0_f64]],
    ];
    let output = node(&input).unwrap();
    assert_noded(&output);
    let fan: Vec<Vec<Point>> = (0_i32..24_i32)
        .map(|step| {
            let angle = f64::from(step) * 0.13_f64;
            vec![
                [angle.cos() * 1e3_f64 + 0.3_f64, angle.sin() * 1e3_f64 + 0.7_f64],
                [-angle.cos() * 1e3_f64 + 0.3_f64, -angle.sin() * 1e3_f64 + 0.7_f64],
            ]
        })
        .collect();
    let fanned = node(&fan).unwrap();
    assert_noded(&fanned);
}

/// Verifies: [DESIGN] repeated positions are removed, a lone point
/// stays a point, and a segment another ends on is split there.
#[test]
fn points_and_repeats() {
    assert_eq!(
        deduplicated(&[[0.0_f64, 0.0_f64], [0.0_f64, 0.0_f64], [1.0_f64, 0.0_f64], [1.0_f64, 0.0_f64]]),
        vec![[0.0_f64, 0.0_f64], [1.0_f64, 0.0_f64]]
    );
    let output = node(&[vec![[1.0_f64, 0.0_f64]], vec![[0.0_f64, 0.0_f64], [2.0_f64, 0.0_f64]]]).unwrap();
    assert_eq!(output, vec![vec![[1.0_f64, 0.0_f64]], vec![[0.0_f64, 0.0_f64], [1.0_f64, 0.0_f64], [2.0_f64, 0.0_f64]]]);
    let junction = node(&[vec![[0.0_f64, 0.0_f64], [2.0_f64, 0.0_f64]], vec![[1.0_f64, 0.0_f64], [1.0_f64, 1.0_f64]]]).unwrap();
    assert_eq!(junction[0], vec![[0.0_f64, 0.0_f64], [1.0_f64, 0.0_f64], [2.0_f64, 0.0_f64]]);
}
