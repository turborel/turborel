//! \[EXTENSION\] The sums distances are made of, for two vectors of one
//! element format: dense pairs through the kernels (`kernels.rs`), and a
//! sparse vector beside a dense or a sparse one by merging their indexes
//! in `O(stored_a + stored_b)` (design document R4.2, R6.4), in `f64`.

use std::cmp::Ordering;

use super::float::wide;
use super::kernels::{self, Half, Lane, Single};
use super::{Element, VectorRef};

/// A sum over the pairs of elements.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Term {
    /// `Σ a_i b_i`.
    Dot,
    /// `Σ (a_i - b_i)^2`.
    Square,
    /// `Σ |a_i - b_i|`.
    Gap,
    /// The count of `i` where `a_i` and `b_i` differ.
    Differ,
}

fn floats<L: Lane>(term: Term, a: &[u8], b: &[u8]) -> f64 {
    match term {
        Term::Dot => f64::from(kernels::fold::<L>(a, b, kernels::product)),
        Term::Square => f64::from(kernels::fold::<L>(a, b, kernels::square)),
        Term::Gap => f64::from(kernels::fold::<L>(a, b, kernels::gap)),
        Term::Differ => wide(i64::try_from(kernels::differing(a, b, L::WIDTH)).unwrap_or(i64::MAX)),
    }
}

fn doubles(term: Term, a: &[u8], b: &[u8]) -> f64 {
    match term {
        Term::Dot => kernels::fold_f64(a, b, |p, q| p * q),
        Term::Square => kernels::fold_f64(a, b, |p, q| (p - q) * (p - q)),
        Term::Gap => kernels::fold_f64(a, b, |p, q| (p - q).abs()),
        Term::Differ => count(kernels::differing(a, b, 8)),
    }
}

fn bytes(term: Term, a: &[u8], b: &[u8]) -> f64 {
    match term {
        Term::Dot => wide(kernels::fold_i8(a, b, kernels::i8_product)),
        Term::Square => wide(kernels::fold_i8(a, b, kernels::i8_square)),
        Term::Gap => wide(kernels::fold_i8(a, b, kernels::i8_gap)),
        Term::Differ => count(kernels::differing(a, b, 1)),
    }
}

fn bits(term: Term, a: &[u8], b: &[u8]) -> f64 {
    // For bits, a square and a gap are both the exclusive or.
    match term {
        Term::Dot => count(kernels::bits(a, b, |p, q| p & q)),
        Term::Square | Term::Gap | Term::Differ => count(kernels::bits(a, b, |p, q| p ^ q)),
    }
}

fn count(n: u64) -> f64 {
    wide(i64::try_from(n).unwrap_or(i64::MAX))
}

/// `term` summed over `a` and `b`, of one element format and dimension
/// count.
pub(crate) fn sum(term: Term, a: VectorRef<'_>, b: VectorRef<'_>) -> f64 {
    if a.indices.is_some() || b.indices.is_some() {
        return merged(term, a, b);
    }
    match a.element {
        Element::Float32 => floats::<Single>(term, a.bytes, b.bytes),
        Element::Float16 => floats::<Half>(term, a.bytes, b.bytes),
        Element::Float64 => doubles(term, a.bytes, b.bytes),
        Element::Int8 => bytes(term, a.bytes, b.bytes),
        Element::Binary => bits(term, a.bytes, b.bytes),
    }
}

/// Which cursor a merge step moves.
enum Step {
    Left,
    Right,
    Both,
}

fn apply(term: Term, p: f64, q: f64) -> f64 {
    match term {
        Term::Dot => p * q,
        Term::Square => (p - q) * (p - q),
        Term::Gap => (p - q).abs(),
        Term::Differ => f64::from(u8::from(p.to_bits() != q.to_bits())),
    }
}

/// `term` summed by merging the stored dimensions of `a` and `b`.
fn merged(term: Term, a: VectorRef<'_>, b: VectorRef<'_>) -> f64 {
    let (mut at_a, mut at_b) = (0_u32, 0_u32);
    let (stored_a, stored_b) = (a.stored(), b.stored());
    let mut total = 0.0_f64;
    loop {
        let left = if at_a < stored_a { a.index(at_a) } else { None };
        let right = if at_b < stored_b { b.index(at_b) } else { None };
        let step = match (left, right) {
            (None, None) => return total,
            (Some(_), None) => Step::Left,
            (None, Some(_)) => Step::Right,
            (Some(dim_a), Some(dim_b)) => match dim_a.cmp(&dim_b) {
                Ordering::Less => Step::Left,
                Ordering::Greater => Step::Right,
                Ordering::Equal => Step::Both,
            },
        };
        let pair = match step {
            Step::Left => (a.value(at_a).unwrap_or_default(), 0.0),
            Step::Right => (0.0, b.value(at_b).unwrap_or_default()),
            Step::Both => (a.value(at_a).unwrap_or_default(), b.value(at_b).unwrap_or_default()),
        };
        total += apply(term, pair.0, pair.1);
        if matches!(step, Step::Left | Step::Both) {
            at_a = at_a.saturating_add(1);
        }
        if matches!(step, Step::Right | Step::Both) {
            at_b = at_b.saturating_add(1);
        }
    }
}

/// The L2 norm of `v`: the image's, but for FLOAT64, whose norm the
/// image holds only to `f32` precision.
pub(crate) fn norm(v: VectorRef<'_>) -> f64 {
    match (v.element, v.norm) {
        (Element::Float64, _) | (_, None) => sum(Term::Dot, v, v).sqrt(),
        (Element::Float32 | Element::Float16 | Element::Int8 | Element::Binary, Some(n)) => f64::from(n),
    }
}

/// The count of bits set in a BINARY vector `a` and `b`, and in `a` or
/// `b`.
pub(crate) fn and_or(a: VectorRef<'_>, b: VectorRef<'_>) -> (f64, f64) {
    (count(kernels::bits(a.bytes, b.bytes, |p, q| p & q)), count(kernels::bits(a.bytes, b.bytes, |p, q| p | q)))
}
