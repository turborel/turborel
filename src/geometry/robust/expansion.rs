//! Exact orientation signs from floating-point expansions, without
//! allocation. When the four coordinate differences of a determinant are
//! exact, each of its two products splits exactly into a rounded product
//! and its rounding error, the error found by one fused multiply-add. The
//! determinant is then exactly a four-term expansion of their difference,
//! and its sign is that of its largest nonzero term (Shewchuk, "Adaptive
//! Precision Floating-Point Arithmetic and Fast Robust Geometric
//! Predicates", Discrete Comput. Geom. 18(3), 1997, §2: Two-Sum,
//! Two-Diff and nonoverlapping expansions). Integer and other grid-aligned
//! coordinates, where degenerate triples are common, take this path. An
//! inexact difference, or a product whose error could underflow or whose
//! sums could overflow, gives no sign: the caller keeps its intervals and
//! exact integers.

use std::cmp::Ordering;

use crate::geometry::shape::Point;

/// IEEE 754 binary64: 52 fraction bits below the exponent field.
const FRACTION_BITS: u32 = 52;
/// The biased exponent of 2^-969. A product at least this large has two
/// factors whose exponents sum to at least -970, so its rounding error is
/// a multiple of 2^-1074 and a binary64 number: one fused multiply-add
/// gives it exactly.
const LEAST_BIASED: u64 = 54;
/// The least product whose error is certainly exact: 2^-969.
const LEAST_PRODUCT: f64 = f64::from_bits(LEAST_BIASED << FRACTION_BITS);
/// Products at most this large leave the expansion's sums finite.
const GREATEST_PRODUCT: f64 = f64::MAX / 8.0;

/// The sign of `(a - c) x (b - c)`, if both products are exact: each
/// factor an exact difference, or either one an exact zero, as on an
/// axis-parallel edge; otherwise none.
pub(super) fn sign([ax, ay]: Point, [bx, by]: Point, [cx, cy]: Point) -> Option<Ordering> {
    let left = product(difference(ax, cx), difference(by, cy))?;
    let right = product(difference(ay, cy), difference(bx, cx))?;
    Some(difference_sign(left, right))
}

/// The exact product of two differences: zero if either is an exact zero,
/// whatever the other, else their split product if both are exact.
fn product(a: Option<f64>, b: Option<f64>) -> Option<(f64, f64)> {
    let zero = |factor: Option<f64>| factor.is_some_and(|x| x.abs() <= 0.0_f64);
    if zero(a) || zero(b) {
        return Some((0.0_f64, 0.0_f64));
    }
    split(a?, b?)
}

/// `a - b`, if it is exact.
fn difference(a: f64, b: f64) -> Option<f64> {
    let (x, error) = two_diff(a, b);
    (x.is_finite() && error.abs() <= 0.0_f64).then_some(x)
}

/// `a * b` as a rounded product and its exact error, if the error is
/// certainly a binary64 number and later sums stay finite.
fn split(a: f64, b: f64) -> Option<(f64, f64)> {
    let p = a * b;
    if p.abs() <= 0.0_f64 {
        // A zero product is exact only if a factor is zero, not if it
        // underflowed.
        return (a.abs() <= 0.0_f64 || b.abs() <= 0.0_f64).then_some((0.0_f64, 0.0_f64));
    }
    let magnitude = p.abs();
    (LEAST_PRODUCT..=GREATEST_PRODUCT).contains(&magnitude).then(|| (p, a.mul_add(b, -p)))
}

/// The sign of `(a1 + a0) - (b1 + b0)`, two nonoverlapping two-term
/// expansions: Shewchuk's Two-Two-Diff, exact, then its largest term.
fn difference_sign((a1, a0): (f64, f64), (b1, b0): (f64, f64)) -> Ordering {
    let (j, zero, x0) = two_one_diff(a1, a0, b0);
    let (x3, x2, x1) = two_one_diff(j, zero, b1);
    let largest = [x3, x2, x1, x0].into_iter().find(|x| x.abs() > 0.0_f64);
    largest.map_or(Ordering::Equal, |x| if x > 0.0_f64 { Ordering::Greater } else { Ordering::Less })
}

/// `(a1 + a0) - b` as a three-term expansion, greatest first.
fn two_one_diff(a1: f64, a0: f64, b: f64) -> (f64, f64, f64) {
    let (i, x0) = two_diff(a0, b);
    let (x2, x1) = two_sum(a1, i);
    (x2, x1, x0)
}

/// `a + b` rounded, and its exact error (Knuth's Two-Sum).
fn two_sum(a: f64, b: f64) -> (f64, f64) {
    let x = a + b;
    let b_virtual = x - a;
    let a_virtual = x - b_virtual;
    (x, (a - a_virtual) + (b - b_virtual))
}

/// `a - b` rounded, and its exact error.
fn two_diff(a: f64, b: f64) -> (f64, f64) {
    let x = a - b;
    let b_virtual = a - x;
    let a_virtual = x + b_virtual;
    (x, (a - a_virtual) + (b_virtual - b))
}

#[cfg(test)]
mod tests;
