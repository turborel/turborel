//! \[DESIGN\] Distance kernels over dense vectors' packed bytes of the
//! formats the database's kernels leave out (design document §6.6):
//! FLOAT64 in sixteen `f64` lanes, each with its own accumulator, over
//! fixed-size chunks, then a scalar tail; BINARY counted bits over 64-bit
//! words. FLOAT32, FLOAT16 and INT8 are summed by the database's kernels
//! (`Kernels::distance_packed`, `distance_i8`, RULES.md D571), whose
//! portable bodies these follow. No `unsafe` and no intrinsics (RULES.md
//! R-SAFE-1). The order of every sum is fixed, so a distance is the same
//! on every run.

use crate::kernels::DistanceTerm;
use crate::kernels::portable::distance::distance_f32;

/// The lanes of a kernel.
const LANES: usize = 16;
/// The bytes of a binary word counted at once.
const WORD: usize = 8;

/// `v·v`, summed as `Kernels::distance_f32` sums a dot product. It is the
/// squared norm a vector index keeps with each vector it holds, and works
/// out once for a query, so a metric measured by norms (COSINE) does not
/// sum them at every call, and gives bit for bit the distance it gave
/// summing them.
#[inline]
pub(crate) fn squared_norm(v: &[f32]) -> f32 {
    distance_f32(v, v, DistanceTerm::Dot)
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
