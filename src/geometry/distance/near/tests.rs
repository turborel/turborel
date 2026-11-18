//! The expanding search checked bit for bit against the nested loops.

use super::*;
use crate::geometry::distance::minimum;

/// The least measure over every pair, as the nested loops take it.
fn nested(left: &[Point], right: &[Point]) -> f64 {
    minimum(segments(left).flat_map(|(a, b)| segments(right).map(move |(c, d)| between(a, b, c, d))))
}

/// A fixed sequence, the same every run (R-TEST-5).
fn next(seed: &mut u64) -> u64 {
    *seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
    *seed >> 33_u32
}

/// A random walk of `steps` points from `start`.
fn walk(seed: &mut u64, start: Point, steps: u32, unit: f64) -> Vec<Point> {
    let mut point = start;
    let mut chain = vec![point];
    for _ in 0..steps {
        let dx = f64::from(u32::try_from(next(seed) % 9).unwrap()) - 4.0_f64;
        let dy = f64::from(u32::try_from(next(seed) % 9).unwrap()) - 4.0_f64;
        point = [point[0] + dx * unit, point[1] + dy * unit];
        chain.push(point);
    }
    chain
}

/// Verifies: [DESIGN] D85: the expanding search returns, to the bit, the
/// least measure the nested loops return, for walks apart and interleaved
/// on integer, fractional, tiny and large scales.
#[test]
fn searches_return_the_nested_minimum_to_the_bit() {
    let mut seed = 9_u64;
    for round in 0..300_u32 {
        let unit = [1.0_f64, 0.37_f64, 1.0e-9_f64, 1.0e9_f64][usize::try_from(round % 4).unwrap()];
        let gap = f64::from(round % 7) * unit * 3.0_f64;
        let left = walk(&mut seed, [0.0_f64, 0.0_f64], 80, unit);
        let right = walk(&mut seed, [gap, gap * 0.5_f64], 80, unit);
        let found = nearest(&left, &right).expect("a search");
        assert_eq!(found.to_bits(), nested(&left, &right).to_bits(), "round {round}");
    }
}
