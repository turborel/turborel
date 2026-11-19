//! Bucketed edge intersection checked against the nested loops it
//! replaces, on random chains and on the degenerate meetings the exact
//! segment test must find.

use super::*;
use crate::geometry::crossing::segments;

/// Whether any pair of edges meets, by the nested loops.
fn nested(left: &[&[Point]], right: &[&[Point]]) -> bool {
    left.iter().any(|a| right.iter().any(|b| segments(a).any(|(p, q)| segments(b).any(|(r, s)| segments_intersect(p, q, r, s)))))
}

/// A fixed sequence, the same every run (R-TEST-5).
fn next(seed: &mut u64) -> u64 {
    *seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
    *seed >> 33_u32
}

/// A random walk of `steps` points from `start`, on a grid of `unit`.
fn walk(seed: &mut u64, start: Point, steps: u32, unit: f64) -> Vec<Point> {
    let mut point = start;
    let mut chain = vec![point];
    for _ in 0..steps {
        let dx = f64::from(u32::try_from(next(seed) % 7).unwrap()) - 3.0_f64;
        let dy = f64::from(u32::try_from(next(seed) % 7).unwrap()) - 3.0_f64;
        point = [point[0] + dx * unit, point[1] + dy * unit];
        chain.push(point);
    }
    chain
}

/// Verifies: [DESIGN] D85: bucketed edge intersection answers as the
/// nested loops do for random walks on integer and fractional grids,
/// where crossings, shared vertices and collinear overlaps are common,
/// whenever it answers at all.
#[test]
fn buckets_agree_with_nested_loops() {
    let mut seed = 5_u64;
    let (mut answered, mut met) = (0_u32, 0_u32);
    for round in 0..400_u32 {
        let unit = if round % 2 == 0 { 1.0_f64 } else { 0.37_f64 };
        let left = walk(&mut seed, [0.0_f64, 0.0_f64], 60, unit);
        let offset = f64::from(round % 9) * unit;
        let right = walk(&mut seed, [offset, -offset], 60, unit);
        let (left, right) = ([left.as_slice()], [right.as_slice()]);
        let expected = nested(&left, &right);
        if let Some(found) = any_crossing(&left, &right) {
            assert_eq!(found, expected, "round {round}");
            answered = answered.saturating_add(1);
            met = met.saturating_add(u32::from(found));
        }
    }
    assert!(answered > 300, "answered {answered}");
    assert!(met > 0 && met < answered, "both outcomes occur: {met} of {answered}");
}

/// Verifies: [DESIGN] D85: meetings at one point are found: an endpoint
/// on the other's interior, shared endpoints, and collinear overlap; and
/// interleaved combs whose envelopes overlap but whose edges never meet
/// are not.
#[test]
fn touching_edges_meet_and_interleaved_combs_do_not() {
    let tee = ([[0.0_f64, 0.0_f64], [10.0_f64, 0.0_f64]], [[5.0_f64, 0.0_f64], [5.0_f64, 7.0_f64]]);
    let shared = ([[0.0_f64, 0.0_f64], [3.0_f64, 3.0_f64]], [[3.0_f64, 3.0_f64], [9.0_f64, 1.0_f64]]);
    let overlap = ([[0.0_f64, 0.0_f64], [6.0_f64, 0.0_f64]], [[4.0_f64, 0.0_f64], [9.0_f64, 0.0_f64]]);
    for (a, b) in [tee, shared, overlap] {
        assert_ne!(any_crossing(&[&a[..]], &[&b[..]]), Some(false), "a meeting is never denied");
    }
    let square = |low: f64, high: f64| vec![[low, low], [high, low], [high, high], [low, high], [low, low]];
    let (first, beside) = (square(0.0_f64, 10.0_f64), [[10.0_f64, 2.0_f64], [20.0_f64, 2.0_f64], [20.0_f64, 9.0_f64], [10.0_f64, 9.0_f64]]);
    assert_eq!(any_crossing(&[&first[..]], &[&beside[..]]), Some(true), "rings sharing part of a side meet");
    // Teeth rise from y = 0 to 10 and hang from y = 20 to 5, interleaved
    // by half a period: the envelopes overlap from y = 5 to 10, the edges
    // never meet.
    let comb = |shift: f64, base: f64, tip: f64| -> Vec<Point> {
        let mut teeth = Vec::new();
        for k in 0..100_u32 {
            let x = f64::from(k) * 2.0_f64 + shift;
            teeth.extend([[x, base], [x, tip], [x + 0.5_f64, tip], [x + 0.5_f64, base]]);
        }
        teeth
    };
    let (up, across) = (comb(0.0_f64, 0.0_f64, 10.0_f64), comb(1.0_f64, 20.0_f64, 5.0_f64));
    assert_eq!(any_crossing(&[&up[..]], &[&across[..]]), Some(false));
    assert!(!nested(&[&up[..]], &[&across[..]]));
}
