//! \[DESIGN\] Kernels on a processor of no vector set these kernels know
//! (RULES.md D570): every kernel its portable body, the one level had.

use turborel::hash::xxhash::{ACC_NB, Scalar, Stripe, Stripes as _, keyed};
use turborel::kernels::{AlpScale, DistanceTerm, Given, Packed, portable};
use turborel::value::CompOp;

use crate::Level;

/// The instructions a `Simd` runs on: the portable bodies alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Isa {
    /// The portable bodies.
    Portable,
}

/// The portable set.
pub(crate) const PORTABLE: Isa = Isa::Portable;

/// The instructions of `level`: the portable ones alone are had.
pub(crate) fn isa(level: Level) -> Option<Isa> {
    match level {
        Level::Portable => Some(Isa::Portable),
        Level::Sse2 | Level::Avx2 | Level::Avx512 | Level::Neon => None,
    }
}

/// `Kernels::select_i64`.
pub(crate) fn select_i64(_isa: Isa, values: &[i64], pred: (CompOp, i64), given: Given<'_>, out: &mut Vec<u32>) {
    portable::select::select_i64(values, pred, given, out);
}

/// `Kernels::select_f64`.
pub(crate) fn select_f64(_isa: Isa, values: &[f64], pred: (CompOp, f64), given: Given<'_>, out: &mut Vec<u32>) {
    portable::select::select_f64(values, pred, given, out);
}

/// `Kernels::select_mask`.
pub(crate) fn select_mask(_isa: Isa, bits: &[u64], given: Given<'_>, out: &mut Vec<u32>) {
    portable::select::select_mask(bits, given, out);
}

/// `Kernels::select_codes`.
pub(crate) fn select_codes(_isa: Isa, codes: &[u32], keep: &[u64], given: Given<'_>, out: &mut Vec<u32>) {
    portable::select::select_codes(codes, keep, given, out);
}

/// `Kernels::sum_i64`.
pub(crate) fn sum_i64(_isa: Isa, values: &[i64], sel: Option<&[u32]>) -> i128 {
    portable::sum::sum_i64(values, sel)
}

/// `Kernels::min_max_i64`.
pub(crate) fn min_max_i64(_isa: Isa, values: &[i64], given: Given<'_>) -> Option<(i64, i64)> {
    portable::sum::min_max_i64(values, given)
}

/// `Kernels::bloom_probe`.
pub(crate) fn bloom_probe(_isa: Isa, words: &[u64], hashes: &[u64], out: &mut Vec<u32>) {
    portable::bloom::bloom_probe(words, hashes, out);
}

/// `Kernels::prefetch_u64` and `prefetch_u32`: no instruction asks.
pub(crate) fn prefetch<T>(values: &[T], at: &[u32]) {
    let _ = (values, at);
}

/// `Kernels::gather_i64`.
pub(crate) fn gather_i64(_isa: Isa, values: &[i64], sel: &[u32], out: &mut Vec<i64>) {
    portable::gather::gather_i64(values, sel, out);
}

/// `Kernels::hash_i64`: the portable body.
pub(crate) fn hash_i64(_isa: Isa, values: &[i64], seed: u64, sel: Option<&[u32]>, out: &mut Vec<u64>) {
    portable::hash::hash_i64(values, seed, sel, out);
}

/// `Kernels::combine_i64`: the portable body.
pub(crate) fn combine_i64(_isa: Isa, hashes: &mut [u64], next: &[u64], seed: u64) {
    portable::hash::combine_i64(hashes, next, seed);
}

/// `Kernels::alp_decode`.
pub(crate) fn alp_decode(_isa: Isa, words: &mut [i64], scale: AlpScale) {
    portable::alp::alp_decode(words, scale);
}

/// `Kernels::normalize_i64`.
pub(crate) fn normalize_i64(
    _isa: Isa,
    given: (&[i64], &[u32]),
    shape: (i64, usize, bool),
    place: (usize, usize),
    keys: &mut [u8],
) -> Option<()> {
    portable::normal::normalize_i64(given, shape, place, keys)
}

/// `Kernels::xxh3_accumulate`.
pub(crate) fn xxh3_accumulate(_isa: Isa, acc: &mut [u64; ACC_NB], stripes: &[Stripe], secret: &[u8]) {
    Scalar.accumulate(acc, keyed(stripes, secret), secret);
}

/// `Kernels::xxh3_scramble`.
pub(crate) fn xxh3_scramble(_isa: Isa, acc: &mut [u64; ACC_NB], key: &Stripe) {
    Scalar.scramble(acc, key);
}

/// `Kernels::distance_f32`.
pub(crate) fn distance_f32(_isa: Isa, a: &[f32], b: &[f32], term: DistanceTerm) -> f32 {
    portable::distance::distance_f32(a, b, term)
}

/// `Kernels::cosine_f32`.
pub(crate) fn cosine_f32(_isa: Isa, a: &[f32], b: &[f32]) -> (f32, f32, f32) {
    portable::distance::cosine_f32(a, b)
}

/// `Kernels::distance_packed`.
pub(crate) fn distance_packed(_isa: Isa, a: &[u8], b: &[u8], shape: (Packed, DistanceTerm)) -> f32 {
    portable::distance::distance_packed(a, b, shape)
}

/// `Kernels::distance_i8`.
pub(crate) fn distance_i8(_isa: Isa, a: &[u8], b: &[u8], term: DistanceTerm) -> i64 {
    portable::distance::distance_i8(a, b, term)
}

/// `Kernels::crc32c`.
pub(crate) fn crc32c(_isa: Isa, register: u32, data: &[u8]) -> u32 {
    portable::crc::crc32c(register, data)
}

/// `Kernels::find_bytes`.
pub(crate) fn find_bytes(_isa: Isa, haystack: &[u8], needle: &[u8]) -> Option<usize> {
    portable::find::find_bytes(haystack, needle)
}

/// `Kernels::utf8`: by the portable body (RULES.md D1175).
pub(crate) fn utf8(_isa: Isa, bytes: &[u8]) -> Option<&str> {
    std::str::from_utf8(bytes).ok()
}
