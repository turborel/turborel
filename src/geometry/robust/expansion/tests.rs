//! Expansion signs checked against the exact integer determinant.

use super::*;
use crate::geometry::robust::exact;

/// A fixed sequence, the same every run (R-TEST-5).
fn next(seed: &mut u64) -> u64 {
    *seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
    *seed >> 11_u32
}

/// Verifies: [DESIGN] D85: on integer and binary-fraction grids, where
/// repeated and collinear points are common, the expansion gives a sign
/// for every triple, and it is the exact integer determinant's; collinear
/// triples on a line give zero.
#[test]
fn grid_orientations_take_the_expansion() {
    let mut seed = 3_u64;
    for scale in [1.0_f64, 1.0_f64 / 1_024.0_f64, 1_048_576.0_f64] {
        for _ in 0_u32..20_000_u32 {
            let collinear = next(&mut seed).is_multiple_of(4);
            let mut coordinate = || f64::from(u32::try_from(next(&mut seed) % 4_096).unwrap()) * scale - 2_048.0_f64 * scale;
            let (a, b, other) = ([coordinate(), coordinate()], [coordinate(), coordinate()], [coordinate(), coordinate()]);
            let c = if collinear { [2.0_f64 * b[0] - a[0], 2.0_f64 * b[1] - a[1]] } else { other };
            assert_eq!(sign(a, b, c), Some(exact(a, b, c)), "{a:?} {b:?} {c:?}");
        }
    }
}

/// Verifies: [DESIGN] D85: where both products round to one number, their
/// errors decide the sign. By Cassini's identity, (F(n+1), F(n)) and
/// (F(n), F(n-1)) about the origin have determinant (-1)^n while both
/// products pass 2^53; offsets keep the differences exact.
#[test]
fn cancelled_products_take_their_errors_sign() {
    let fibonacci = [63_245_986_u32, 102_334_155, 165_580_141, 267_914_296, 433_494_437];
    for (n, window) in (40_u32..).zip(fibonacci.windows(3)) {
        let [before, at, after] = [window[0], window[1], window[2]].map(f64::from);
        let expected = if n % 2 == 0 { Ordering::Greater } else { Ordering::Less };
        for offset in [0.0_f64, 1_048_576.0_f64, -3.0_f64] {
            let (a, b, o) = ([after + offset, at + offset], [at + offset, before + offset], [offset, offset]);
            assert_eq!(sign(a, b, o), Some(expected), "n {n} offset {offset}");
            assert_eq!(exact(a, b, o), expected);
        }
    }
}

/// Verifies: [DESIGN] D85: for any binary64 coordinates the expansion
/// either gives the exact sign or none, never a wrong one: inexact
/// differences, products near underflow or overflow, repeated points and
/// points sharing an abscissa, across every exponent; the last two are
/// always decided.
#[test]
fn expansion_signs_are_exact_or_absent() {
    let mut seed = 11_u64;
    for _ in 0_u32..50_000_u32 {
        let mut point = || {
            std::array::from_fn(|_| {
                let bits = (next(&mut seed) << 11_u32) | (next(&mut seed) & 0x7ff);
                let value = f64::from_bits(bits);
                if value.is_finite() { value } else { 1.0_f64 }
            })
        };
        let (a, b, c) = (point(), point(), point());
        for (p, q, r) in [(a, b, c), (a, a, c)] {
            if let Some(found) = sign(p, q, r) {
                assert_eq!(found, exact(p, q, r), "{p:?} {q:?} {r:?}");
            }
        }
        let vertical = ([a[0], b[1]], [a[0], c[1]]);
        assert_eq!(sign(a, a, a), Some(Ordering::Equal), "{a:?}");
        assert_eq!(sign(a, vertical.0, vertical.1), Some(Ordering::Equal), "{a:?} {vertical:?}");
    }
}
