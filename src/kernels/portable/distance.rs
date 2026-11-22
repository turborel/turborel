//! \[DESIGN\] The portable bodies of the distance kernels over dense
//! vectors (RULES.md D354, D571), one compiled for each element format
//! and term so the compiler vectorizes it (vector design document §6.6).
//! Sixteen lanes, each with its own accumulator, run over fixed-size
//! chunks; the lanes are then summed in order and a scalar tail added.
//! No `unsafe` and no intrinsics (RULES.md R-SAFE-1), and the loops are
//! written so bounds checks fold away. FLOAT32 and FLOAT16 accumulate in
//! `f32`, INT8 in `i32` lanes (R3.10). The order of every sum is fixed, so
//! a distance is the same on every run and at every instruction set. A
//! body of another set sums each lane alone over the same chunks, then
//! ends as these do (`finish_f32`, `finish_packed`, `finish_cosine`,
//! `finish_i8`).

use crate::kernels::{DistanceTerm, Packed};
use crate::vector::half;

/// The lanes of a distance kernel.
pub const LANES: usize = 16;

/// The `f32` accumulators of the lanes.
pub type Lanes = [f32; LANES];

/// The `i32` accumulators of INT8's lanes.
pub type ByteLanes = [i32; LANES];

/// The bytes of `LANES` FLOAT32 elements.
pub const SINGLE_STEP: usize = 64;

/// The bytes of `LANES` FLOAT16 elements.
pub const HALF_STEP: usize = 32;

/// An element format read as `f32`.
trait Lane {
    /// Its bytes.
    const WIDTH: usize;
    /// The bytes of `LANES` of it.
    const STEP: usize;
    /// The element `bytes` holds, `WIDTH` long.
    fn load(bytes: &[u8]) -> f32;
}

/// FLOAT32.
struct Single;
/// FLOAT16.
struct Half;

impl Lane for Single {
    const WIDTH: usize = 4;
    const STEP: usize = SINGLE_STEP;
    #[inline]
    fn load(bytes: &[u8]) -> f32 {
        <[u8; 4]>::try_from(bytes).map_or(0.0, f32::from_le_bytes)
    }
}

impl Lane for Half {
    const WIDTH: usize = 2;
    const STEP: usize = HALF_STEP;
    #[inline]
    fn load(bytes: &[u8]) -> f32 {
        <[u8; 2]>::try_from(bytes).map_or(0.0, |b| half(u16::from_le_bytes(b)))
    }
}

/// `p * q`.
#[inline]
fn product(p: f32, q: f32) -> f32 {
    p * q
}

/// `(p - q)^2`.
#[inline]
fn square(p: f32, q: f32) -> f32 {
    let d = p - q;
    d * d
}

/// `|p - q|`.
#[inline]
fn gap(p: f32, q: f32) -> f32 {
    (p - q).abs()
}

/// The product of two INT8 elements.
#[inline]
fn i8_product(p: i32, q: i32) -> i32 {
    p.wrapping_mul(q)
}

/// The square of the difference of two INT8 elements.
#[inline]
fn i8_square(p: i32, q: i32) -> i32 {
    let d = p.wrapping_sub(q);
    d.wrapping_mul(d)
}

/// The magnitude of the difference of two INT8 elements.
#[inline]
fn i8_gap(p: i32, q: i32) -> i32 {
    p.wrapping_sub(q).wrapping_abs()
}

/// `term`'s function of two `f32`s.
fn term_of(term: DistanceTerm) -> fn(f32, f32) -> f32 {
    match term {
        DistanceTerm::Dot => product,
        DistanceTerm::Square => square,
        DistanceTerm::Gap => gap,
    }
}

/// `term`'s function of two INT8 elements.
fn i8_term_of(term: DistanceTerm) -> fn(i32, i32) -> i32 {
    match term {
        DistanceTerm::Dot => i8_product,
        DistanceTerm::Square => i8_square,
        DistanceTerm::Gap => i8_gap,
    }
}

/// A chunk's lanes.
#[inline]
fn lanes<L: Lane>(chunk: &[u8]) -> Lanes {
    let mut out = [0.0_f32; LANES];
    for (slot, bytes) in out.iter_mut().zip(chunk.chunks_exact(L::WIDTH)) {
        *slot = L::load(bytes);
    }
    out
}

/// The lanes summed in a fixed order.
#[inline]
fn total(acc: Lanes) -> f32 {
    acc.iter().fold(0.0_f32, |sum, x| sum + x)
}

// ---------------------------------------------------------------------------
// FLOAT32 and FLOAT16 packed in bytes
// ---------------------------------------------------------------------------

/// `Kernels::distance_packed`: the sum over each pair of elements of
/// `format` of `term`.
pub fn distance_packed(a: &[u8], b: &[u8], (format, term): (Packed, DistanceTerm)) -> f32 {
    match format {
        Packed::Single => packed_by::<Single>(a, b, term),
        Packed::Half => packed_by::<Half>(a, b, term),
    }
}

/// `distance_packed` of one format.
fn packed_by<L: Lane>(a: &[u8], b: &[u8], term: DistanceTerm) -> f32 {
    match term {
        DistanceTerm::Dot => fold::<L>(a, b, product),
        DistanceTerm::Square => fold::<L>(a, b, square),
        DistanceTerm::Gap => fold::<L>(a, b, gap),
    }
}

/// The sum over each pair of elements of `term`.
#[inline]
fn fold<L: Lane>(a: &[u8], b: &[u8], term: impl Fn(f32, f32) -> f32) -> f32 {
    let (ca, cb) = (a.chunks_exact(L::STEP), b.chunks_exact(L::STEP));
    let (ra, rb) = (ca.remainder(), cb.remainder());
    let mut acc = [0.0_f32; LANES];
    for (x, y) in ca.zip(cb) {
        let (x, y) = (lanes::<L>(x), lanes::<L>(y));
        for ((sum, p), q) in acc.iter_mut().zip(x).zip(y) {
            *sum += term(p, q);
        }
    }
    packed_tail::<L>(acc, (ra, rb), term)
}

/// The lanes summed, then the elements of the remainders.
#[inline]
fn packed_tail<L: Lane>(acc: Lanes, (ra, rb): (&[u8], &[u8]), term: impl Fn(f32, f32) -> f32) -> f32 {
    let tail = ra.chunks_exact(L::WIDTH).zip(rb.chunks_exact(L::WIDTH)).fold(0.0_f32, |s, (p, q)| s + term(L::load(p), L::load(q)));
    total(acc) + tail
}

/// The end of `distance_packed` of lanes summed over the whole chunks of
/// `format`'s step (`SINGLE_STEP` or `HALF_STEP`), given the remainders of
/// `a` and `b` past them.
pub fn finish_packed(acc: Lanes, rest: (&[u8], &[u8]), (format, term): (Packed, DistanceTerm)) -> f32 {
    match format {
        Packed::Single => packed_tail::<Single>(acc, rest, term_of(term)),
        Packed::Half => packed_tail::<Half>(acc, rest, term_of(term)),
    }
}

// ---------------------------------------------------------------------------
// f32 slices
// ---------------------------------------------------------------------------

/// `Kernels::distance_f32`: the sum over each pair of `f32`s of `term`.
pub fn distance_f32(a: &[f32], b: &[f32], term: DistanceTerm) -> f32 {
    match term {
        DistanceTerm::Dot => fold_f32(a, b, product),
        DistanceTerm::Square => fold_f32(a, b, square),
        DistanceTerm::Gap => fold_f32(a, b, gap),
    }
}

/// The sum over each pair of `f32`s of `term`.
#[inline]
fn fold_f32(a: &[f32], b: &[f32], term: impl Fn(f32, f32) -> f32) -> f32 {
    let ((ca, ra), (cb, rb)) = (a.as_chunks::<LANES>(), b.as_chunks::<LANES>());
    let mut acc = [0.0_f32; LANES];
    for (x, y) in ca.iter().zip(cb) {
        for ((sum, p), q) in acc.iter_mut().zip(x).zip(y) {
            *sum += term(*p, *q);
        }
    }
    f32_tail(acc, (ra, rb), term)
}

/// The lanes summed, then the remainders' pairs.
#[inline]
fn f32_tail(acc: Lanes, (ra, rb): (&[f32], &[f32]), term: impl Fn(f32, f32) -> f32) -> f32 {
    total(acc) + ra.iter().zip(rb).fold(0.0_f32, |s, (p, q)| s + term(*p, *q))
}

/// The end of `distance_f32` of lanes summed over the whole chunks of
/// `LANES`, given the remainders of `a` and `b` past them.
pub fn finish_f32(acc: Lanes, rest: (&[f32], &[f32]), term: DistanceTerm) -> f32 {
    f32_tail(acc, rest, term_of(term))
}

/// `Kernels::cosine_f32`: `a·a`, `b·b` and `a·b` of `f32`s in one pass,
/// each summed in the order `distance_f32` sums a dot product, so each is
/// bit for bit that call's. COSINE needs all three, and one pass reads
/// each element once where three passes read it three times.
pub fn cosine_f32(a: &[f32], b: &[f32]) -> (f32, f32, f32) {
    let ((ca, ra), (cb, rb)) = (a.as_chunks::<LANES>(), b.as_chunks::<LANES>());
    let (mut aa, mut bb, mut ab) = ([0.0_f32; LANES], [0.0_f32; LANES], [0.0_f32; LANES]);
    for (x, y) in ca.iter().zip(cb) {
        for ((((sa, sb), sab), p), q) in aa.iter_mut().zip(bb.iter_mut()).zip(ab.iter_mut()).zip(x).zip(y) {
            *sa += p * p;
            *sb += q * q;
            *sab += p * q;
        }
    }
    finish_cosine((aa, bb, ab), (ra, rb))
}

/// The end of `cosine_f32` of the lanes of `a·a`, `b·b` and `a·b` summed
/// over the whole chunks of `LANES`, given the remainders past them.
pub fn finish_cosine((aa, bb, ab): (Lanes, Lanes, Lanes), (ra, rb): (&[f32], &[f32])) -> (f32, f32, f32) {
    let tail = |term: fn(&f32, &f32) -> f32| ra.iter().zip(rb).fold(0.0_f32, |s, (p, q)| s + term(p, q));
    (total(aa) + tail(|p, _| p * p), total(bb) + tail(|_, q| q * q), total(ab) + tail(|p, q| p * q))
}

// ---------------------------------------------------------------------------
// INT8
// ---------------------------------------------------------------------------

/// `Kernels::distance_i8`: the sum over each pair of INT8 elements of
/// `term`, in `i32` lanes. Each lane sums at most 4,096 terms of at most
/// 65,025, well inside an `i32`, and the lanes are summed in an `i64`.
pub fn distance_i8(a: &[u8], b: &[u8], term: DistanceTerm) -> i64 {
    match term {
        DistanceTerm::Dot => fold_i8(a, b, i8_product),
        DistanceTerm::Square => fold_i8(a, b, i8_square),
        DistanceTerm::Gap => fold_i8(a, b, i8_gap),
    }
}

/// An INT8 element as an `i32`.
#[inline]
fn load_i8(x: u8) -> i32 {
    i32::from(i8::from_le_bytes([x]))
}

/// The sum over each pair of INT8 elements of `term`.
#[inline]
fn fold_i8(a: &[u8], b: &[u8], term: impl Fn(i32, i32) -> i32) -> i64 {
    let ((ca, ra), (cb, rb)) = (a.as_chunks::<LANES>(), b.as_chunks::<LANES>());
    let mut acc = [0_i32; LANES];
    for (x, y) in ca.iter().zip(cb) {
        for ((sum, p), q) in acc.iter_mut().zip(x).zip(y) {
            *sum = sum.wrapping_add(term(load_i8(*p), load_i8(*q)));
        }
    }
    i8_tail(acc, (ra, rb), term)
}

/// The remainders' pairs, then the lanes added.
#[inline]
fn i8_tail(acc: ByteLanes, (ra, rb): (&[u8], &[u8]), term: impl Fn(i32, i32) -> i32) -> i64 {
    let tail = ra.iter().zip(rb).fold(0_i64, |s, (p, q)| s.wrapping_add(i64::from(term(load_i8(*p), load_i8(*q)))));
    acc.iter().fold(tail, |s, x| s.wrapping_add(i64::from(*x)))
}

/// The end of `distance_i8` of lanes summed over the whole chunks of
/// `LANES`, given the remainders of `a` and `b` past them.
pub fn finish_i8(acc: ByteLanes, rest: (&[u8], &[u8]), term: DistanceTerm) -> i64 {
    i8_tail(acc, rest, i8_term_of(term))
}
