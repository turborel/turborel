//! \[DESIGN\] Distance kernels over dense vectors' packed bytes, one
//! compiled for each element format so the compiler vectorizes it (design
//! document §6.6): sixteen lanes, each with its own accumulator, over
//! fixed-size chunks, then a scalar tail. No `unsafe` and no intrinsics
//! (RULES.md R-SAFE-1): the loops are written so bounds checks fold away.
//! FLOAT32 and FLOAT16 accumulate in `f32`, INT8 in `i32` lanes, FLOAT64
//! in `f64` (R3.10); BINARY counts bits over 64-bit words. The order of
//! every sum is fixed, so a distance is the same on every run.

/// The lanes of a kernel.
pub(crate) const LANES: usize = 16;
/// The bytes of a binary word counted at once.
const WORD: usize = 8;

/// An element format a kernel reads as `f32`.
pub(crate) trait Lane {
    /// Its bytes.
    const WIDTH: usize;
    /// The bytes of `LANES` of it.
    const STEP: usize;
    /// The element `bytes` holds, `WIDTH` long.
    fn load(bytes: &[u8]) -> f32;
}

/// FLOAT32.
pub(crate) struct Single;
/// FLOAT16.
pub(crate) struct Half;

impl Lane for Single {
    const WIDTH: usize = 4;
    const STEP: usize = 64;
    #[inline]
    fn load(bytes: &[u8]) -> f32 {
        <[u8; 4]>::try_from(bytes).map_or(0.0, f32::from_le_bytes)
    }
}

impl Lane for Half {
    const WIDTH: usize = 2;
    const STEP: usize = 32;
    #[inline]
    fn load(bytes: &[u8]) -> f32 {
        <[u8; 2]>::try_from(bytes).map_or(0.0, |b| super::half(u16::from_le_bytes(b)))
    }
}

/// A chunk's lanes.
#[inline]
fn lanes<L: Lane>(chunk: &[u8]) -> [f32; LANES] {
    let mut out = [0.0_f32; LANES];
    for (slot, bytes) in out.iter_mut().zip(chunk.chunks_exact(L::WIDTH)) {
        *slot = L::load(bytes);
    }
    out
}

/// The lanes summed in a fixed order.
#[inline]
fn total(acc: [f32; LANES]) -> f32 {
    acc.iter().fold(0.0_f32, |sum, x| sum + x)
}

/// The sum over each pair of elements of `term`.
#[inline]
pub(crate) fn fold<L: Lane>(a: &[u8], b: &[u8], term: impl Fn(f32, f32) -> f32) -> f32 {
    let (ca, cb) = (a.chunks_exact(L::STEP), b.chunks_exact(L::STEP));
    let (ra, rb) = (ca.remainder(), cb.remainder());
    let mut acc = [0.0_f32; LANES];
    for (x, y) in ca.zip(cb) {
        let (x, y) = (lanes::<L>(x), lanes::<L>(y));
        for ((sum, p), q) in acc.iter_mut().zip(x).zip(y) {
            *sum += term(p, q);
        }
    }
    let tail = ra.chunks_exact(L::WIDTH).zip(rb.chunks_exact(L::WIDTH)).fold(0.0_f32, |s, (p, q)| s + term(L::load(p), L::load(q)));
    total(acc) + tail
}

/// The sum over each pair of `f32`s of `term`, for the quantizers.
#[inline]
pub(crate) fn fold_f32(a: &[f32], b: &[f32], term: impl Fn(f32, f32) -> f32) -> f32 {
    let ((ca, ra), (cb, rb)) = (a.as_chunks::<LANES>(), b.as_chunks::<LANES>());
    let mut acc = [0.0_f32; LANES];
    for (x, y) in ca.iter().zip(cb) {
        for ((sum, p), q) in acc.iter_mut().zip(x).zip(y) {
            *sum += term(*p, *q);
        }
    }
    total(acc) + ra.iter().zip(rb).fold(0.0_f32, |s, (p, q)| s + term(*p, *q))
}

/// `a·a`, `b·b` and `a·b` of `f32`s in one pass, each summed in the
/// order `fold_f32` with `product` sums it, so each is bit for bit that
/// call's. COSINE needs all three, and one pass reads each element once
/// where three passes read it three times.
#[inline]
pub(crate) fn cosine_terms(a: &[f32], b: &[f32]) -> (f32, f32, f32) {
    let ((ca, ra), (cb, rb)) = (a.as_chunks::<LANES>(), b.as_chunks::<LANES>());
    let (mut aa, mut bb, mut ab) = ([0.0_f32; LANES], [0.0_f32; LANES], [0.0_f32; LANES]);
    for (x, y) in ca.iter().zip(cb) {
        for ((((sa, sb), sab), p), q) in aa.iter_mut().zip(bb.iter_mut()).zip(ab.iter_mut()).zip(x).zip(y) {
            *sa += p * p;
            *sb += q * q;
            *sab += p * q;
        }
    }
    let tail = |term: fn(&f32, &f32) -> f32| ra.iter().zip(rb).fold(0.0_f32, |s, (p, q)| s + term(p, q));
    (total(aa) + tail(|p, _| p * p), total(bb) + tail(|_, q| q * q), total(ab) + tail(|p, q| p * q))
}

/// The bytes of `LANES` FLOAT64 elements.
const DOUBLE_STEP: usize = 128;

/// The sum over each pair of FLOAT64 elements of `term`.
#[inline]
pub(crate) fn fold_f64(a: &[u8], b: &[u8], term: impl Fn(f64, f64) -> f64) -> f64 {
    let ((ca, ra), (cb, rb)) = (a.as_chunks::<DOUBLE_STEP>(), b.as_chunks::<DOUBLE_STEP>());
    let mut acc = [0.0_f64; LANES];
    for (x, y) in ca.iter().zip(cb) {
        let (x, y) = (x.as_chunks::<WORD>().0, y.as_chunks::<WORD>().0);
        for ((sum, p), q) in acc.iter_mut().zip(x).zip(y) {
            *sum += term(f64::from_le_bytes(*p), f64::from_le_bytes(*q));
        }
    }
    let (ra, rb) = (ra.as_chunks::<WORD>().0, rb.as_chunks::<WORD>().0);
    let tail = ra.iter().zip(rb).fold(0.0_f64, |s, (p, q)| s + term(f64::from_le_bytes(*p), f64::from_le_bytes(*q)));
    acc.iter().fold(0.0_f64, |s, x| s + x) + tail
}

/// The sum over each pair of INT8 elements of `term`, in `i32` lanes:
/// each lane sums at most 4,096 terms of at most 65,025, well inside an
/// `i32`, and the lanes are summed in an `i64`.
#[inline]
pub(crate) fn fold_i8(a: &[u8], b: &[u8], term: impl Fn(i32, i32) -> i32) -> i64 {
    let load = |x: u8| i32::from(i8::from_le_bytes([x]));
    let ((ca, ra), (cb, rb)) = (a.as_chunks::<LANES>(), b.as_chunks::<LANES>());
    let mut acc = [0_i32; LANES];
    for (x, y) in ca.iter().zip(cb) {
        for ((sum, p), q) in acc.iter_mut().zip(x).zip(y) {
            *sum = sum.wrapping_add(term(load(*p), load(*q)));
        }
    }
    let tail = ra.iter().zip(rb).fold(0_i64, |s, (p, q)| s.wrapping_add(i64::from(term(load(*p), load(*q)))));
    acc.iter().fold(tail, |s, x| s.wrapping_add(i64::from(*x)))
}

/// The product of two INT8 elements.
#[inline]
pub(crate) fn i8_product(p: i32, q: i32) -> i32 {
    p.wrapping_mul(q)
}

/// The square of the difference of two INT8 elements.
#[inline]
pub(crate) fn i8_square(p: i32, q: i32) -> i32 {
    let d = p.wrapping_sub(q);
    d.wrapping_mul(d)
}

/// The magnitude of the difference of two INT8 elements.
#[inline]
pub(crate) fn i8_gap(p: i32, q: i32) -> i32 {
    p.wrapping_sub(q).wrapping_abs()
}

/// The bits set in `a` op `b` over packed bits, 64 at a time.
#[inline]
pub(crate) fn bits(a: &[u8], b: &[u8], op: impl Fn(u64, u64) -> u64) -> u64 {
    let ((ca, ra), (cb, rb)) = (a.as_chunks::<WORD>(), b.as_chunks::<WORD>());
    let words = ca
        .iter()
        .zip(cb)
        .fold(0_u64, |s, (p, q)| s.wrapping_add(u64::from(op(u64::from_le_bytes(*p), u64::from_le_bytes(*q)).count_ones())));
    ra.iter().zip(rb).fold(words, |s, (p, q)| s.wrapping_add(u64::from(op(u64::from(*p), u64::from(*q)).count_ones())))
}

/// The count of elements `width` bytes wide whose bytes differ.
#[inline]
pub(crate) fn differing(a: &[u8], b: &[u8], width: usize) -> u64 {
    let count = a.chunks_exact(width.max(1)).zip(b.chunks_exact(width.max(1))).filter(|(p, q)| p != q).count();
    u64::try_from(count).unwrap_or(u64::MAX)
}

/// `p * q`.
#[inline]
pub(crate) fn product(p: f32, q: f32) -> f32 {
    p * q
}

/// `(p - q)^2`.
#[inline]
pub(crate) fn square(p: f32, q: f32) -> f32 {
    let d = p - q;
    d * d
}

/// `|p - q|`.
#[inline]
pub(crate) fn gap(p: f32, q: f32) -> f32 {
    (p - q).abs()
}
