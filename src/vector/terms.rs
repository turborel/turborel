//! \[EXTENSION\] The sums distances are made of, for two vectors of one
//! element format: dense pairs through the database's kernels (FLOAT32,
//! FLOAT16 and INT8, RULES.md D571) or the vector module's (`kernels.rs`,
//! FLOAT64 and BINARY), and a
//! sparse vector beside a dense or a sparse one by merging their indexes
//! in `O(stored_a + stored_b)` (design document R4.2, R6.4), in `f64`.

use std::cmp::Ordering;

use super::float::wide;
use super::kernels;
use super::{Element, VectorRef};
use crate::kernels::{DistanceTerm, Kernels, Packed};

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

impl Term {
    /// The kernels' term of a sum they take, of all but `Differ`.
    fn summed(self) -> Option<DistanceTerm> {
        match self {
            Term::Dot => Some(DistanceTerm::Dot),
            Term::Square => Some(DistanceTerm::Square),
            Term::Gap => Some(DistanceTerm::Gap),
            Term::Differ => None,
        }
    }
}

/// The bytes of a FLOAT32 element.
const SINGLE_BYTES: usize = 4;
/// The bytes of a FLOAT16 element.
const HALF_BYTES: usize = 2;

fn floats(kernels: &dyn Kernels, (format, term): (Packed, Term), a: &[u8], b: &[u8]) -> f64 {
    let width = match format {
        Packed::Single => SINGLE_BYTES,
        Packed::Half => HALF_BYTES,
    };
    match term.summed() {
        Some(t) => f64::from(kernels.distance_packed(a, b, (format, t))),
        None => wide(i64::try_from(kernels::differing(a, b, width)).unwrap_or(i64::MAX)),
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

fn bytes(kernels: &dyn Kernels, term: Term, a: &[u8], b: &[u8]) -> f64 {
    match term.summed() {
        Some(t) => wide(kernels.distance_i8(a, b, t)),
        None => count(kernels::differing(a, b, 1)),
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
/// count, on `kernels`.
pub(crate) fn sum(kernels: &dyn Kernels, term: Term, a: VectorRef<'_>, b: VectorRef<'_>) -> f64 {
    if a.indices.is_some() || b.indices.is_some() {
        return merged(term, a, b);
    }
    match a.element {
        Element::Float32 => floats(kernels, (Packed::Single, term), a.bytes, b.bytes),
        Element::Float16 => floats(kernels, (Packed::Half, term), a.bytes, b.bytes),
        Element::Float64 => doubles(term, a.bytes, b.bytes),
        Element::Int8 => bytes(kernels, term, a.bytes, b.bytes),
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
/// image holds only to `f32` precision; summed on `kernels`.
pub(crate) fn norm(kernels: &dyn Kernels, v: VectorRef<'_>) -> f64 {
    match (v.element, v.norm) {
        (Element::Float64, _) | (_, None) => sum(kernels, Term::Dot, v, v).sqrt(),
        (Element::Float32 | Element::Float16 | Element::Int8 | Element::Binary, Some(n)) => f64::from(n),
    }
}

/// The count of bits set in a BINARY vector `a` and `b`, and in `a` or
/// `b`.
pub(crate) fn and_or(a: VectorRef<'_>, b: VectorRef<'_>) -> (f64, f64) {
    (count(kernels::bits(a.bytes, b.bytes, |p, q| p & q)), count(kernels::bits(a.bytes, b.bytes, |p, q| p | q)))
}
