//! \[DESIGN\] Kernels on x86-64 (RULES.md D570): the instruction sets a
//! `Simd` may hold, each made only of a level the processor has, and each
//! kernel's choice among its bodies by the set. SSE2 is every x86-64
//! processor's, and the compiler already builds the portable bodies for
//! it, so a kernel no SSE2 instruction speeds runs its portable body
//! there; AVX2 and AVX-512 have their own bodies (`select.rs`, `sum.rs`,
//! `alp.rs`, `distance.rs`), XXH3's loops theirs at each set (`xxh3.rs`),
//! and CRC-32C SSE4.2's `crc32`, which both levels have (`crate::crc`).

use std::arch::x86_64::_mm_crc32_u64;
use turborel::hash::xxhash::{ACC_NB, Scalar, Stripe, Stripes as _, keyed};

use turborel::kernels::portable::distance::{self as distances, HALF_STEP, LANES, SINGLE_STEP};
use turborel::kernels::{AlpScale, DistanceTerm, Given, Packed, portable};
use turborel::value::CompOp;

use crate::Level;

mod alp;
mod bloom;
mod distance;
mod find;
mod hash;
mod select;
mod sum;
mod utf8;
pub(crate) mod xxh3;

/// \[DESIGN\] The instructions a `Simd` runs on: made only by `isa`, of a
/// level the processor has, so a `#[target_feature]` function of its set
/// called on it runs on instructions the processor has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Isa {
    /// The portable bodies.
    Portable,
    /// SSE2.
    Sse2,
    /// AVX2 and x86-64-v3's other instructions.
    Avx2,
    /// AVX-512 F, BW, DQ, VL and CD, and AVX2's.
    Avx512,
}

/// The portable set, which needs no instruction detected.
pub(crate) const PORTABLE: Isa = Isa::Portable;

/// Whether the processor has every instruction of AVX2's level.
fn has_avx2() -> bool {
    std::arch::is_x86_feature_detected!("avx2")
        && std::arch::is_x86_feature_detected!("bmi1")
        && std::arch::is_x86_feature_detected!("bmi2")
        && std::arch::is_x86_feature_detected!("popcnt")
        && std::arch::is_x86_feature_detected!("lzcnt")
        && std::arch::is_x86_feature_detected!("fma")
        && std::arch::is_x86_feature_detected!("f16c")
        && std::arch::is_x86_feature_detected!("sse4.2")
}

/// Whether the processor has every instruction of AVX-512's level.
fn has_avx512() -> bool {
    has_avx2()
        && std::arch::is_x86_feature_detected!("avx512f")
        && std::arch::is_x86_feature_detected!("avx512bw")
        && std::arch::is_x86_feature_detected!("avx512dq")
        && std::arch::is_x86_feature_detected!("avx512vl")
        && std::arch::is_x86_feature_detected!("avx512cd")
}

/// The instructions of `level`, if the processor has them: the only maker
/// of an `Isa` but the portable one.
pub(crate) fn isa(level: Level) -> Option<Isa> {
    match level {
        Level::Portable => Some(Isa::Portable),
        Level::Sse2 => std::arch::is_x86_feature_detected!("sse2").then_some(Isa::Sse2),
        Level::Avx2 => has_avx2().then_some(Isa::Avx2),
        Level::Avx512 => has_avx512().then_some(Isa::Avx512),
        Level::Neon => None,
    }
}

/// `Kernels::select_i64`: of all rows on AVX2 or AVX-512, of a selection
/// by the portable body, whose loads are each a row's.
pub(crate) fn select_i64(isa: Isa, values: &[i64], pred: (CompOp, i64), given: Given<'_>, out: &mut Vec<u32>) {
    let (nulls, None) = given else { return portable::select::select_i64(values, pred, given, out) };
    match isa {
        Isa::Portable | Isa::Sse2 => portable::select::select_i64(values, pred, given, out),
        Isa::Avx2 => select::i64_avx2(values, pred, nulls, out),
        Isa::Avx512 => select::i64_avx512(values, pred, nulls, out),
    }
}

/// `Kernels::select_f64`, as `select_i64`.
pub(crate) fn select_f64(isa: Isa, values: &[f64], pred: (CompOp, f64), given: Given<'_>, out: &mut Vec<u32>) {
    let (nulls, None) = given else { return portable::select::select_f64(values, pred, given, out) };
    match isa {
        Isa::Portable | Isa::Sse2 => portable::select::select_f64(values, pred, given, out),
        Isa::Avx2 => select::f64_avx2(values, pred, nulls, out),
        Isa::Avx512 => select::f64_avx512(values, pred, nulls, out),
    }
}

/// `Kernels::select_mask`, as `select_i64`.
pub(crate) fn select_mask(isa: Isa, bits: &[u64], given: Given<'_>, out: &mut Vec<u32>) {
    let (nulls, None) = given else { return portable::select::select_mask(bits, given, out) };
    match isa {
        Isa::Portable | Isa::Sse2 => portable::select::select_mask(bits, given, out),
        Isa::Avx2 | Isa::Avx512 => select::mask(isa, bits, nulls, out),
    }
}

/// `Kernels::select_codes`: by the portable body at every set, whose
/// lookups of a code's bit a row at a time measured faster than truth
/// words made of the same lookups and written by the set's stores
/// (RULES.md D570).
pub(crate) fn select_codes(_isa: Isa, codes: &[u32], keep: &[u64], given: Given<'_>, out: &mut Vec<u32>) {
    portable::select::select_codes(codes, keep, given, out);
}

/// `Kernels::sum_i64`: of all rows on AVX2 or AVX-512; of a selection
/// that picks at least a quarter of the rows it spans, the loop of all
/// rows masked to the rows picked; of fewer, by the portable body, whose
/// loads are each a picked row's (RULES.md D570).
pub(crate) fn sum_i64(isa: Isa, values: &[i64], sel: Option<&[u32]>) -> i128 {
    match (isa, sel) {
        (Isa::Portable | Isa::Sse2, _) => portable::sum::sum_i64(values, sel),
        (Isa::Avx2, None) => sum::sum_avx2(values),
        (Isa::Avx512, None) => sum::sum_avx512(values),
        (Isa::Avx2, Some(picks)) => sum::sum_picked(sum::Wide::Avx2, values, picks).unwrap_or_else(|| portable::sum::sum_i64(values, sel)),
        (Isa::Avx512, Some(picks)) => {
            sum::sum_picked(sum::Wide::Avx512, values, picks).unwrap_or_else(|| portable::sum::sum_i64(values, sel))
        }
    }
}

/// `Kernels::min_max_i64`, as `sum_i64`.
pub(crate) fn min_max_i64(isa: Isa, values: &[i64], given: Given<'_>) -> Option<(i64, i64)> {
    match (isa, given) {
        (Isa::Portable | Isa::Sse2, _) => portable::sum::min_max_i64(values, given),
        (Isa::Avx2, (nulls, None)) => sum::min_max_avx2(values, nulls),
        (Isa::Avx512, (nulls, None)) => sum::min_max_avx512(values, nulls),
        (Isa::Avx2, (nulls, Some(picks))) => sum::min_max_picked(sum::Wide::Avx2, values, (nulls, picks)),
        (Isa::Avx512, (nulls, Some(picks))) => sum::min_max_picked(sum::Wide::Avx512, values, (nulls, picks)),
    }
}

/// `Kernels::bloom_probe`: four or eight hashes' words and bits a step
/// on AVX2 or AVX-512, each tested with no branch (RULES.md D1087).
pub(crate) fn bloom_probe(isa: Isa, words: &[u64], hashes: &[u64], out: &mut Vec<u32>) {
    match isa {
        Isa::Portable | Isa::Sse2 => portable::bloom::bloom_probe(words, hashes, out),
        Isa::Avx2 => bloom::probe_avx2(words, hashes, out),
        Isa::Avx512 => bloom::probe_avx512(words, hashes, out),
    }
}

/// `Kernels::gather_i64`: by the portable body at every set, whose loads
/// a row at a time measured faster than the vector gathers of AVX2 at
/// every size (RULES.md D570).
pub(crate) fn gather_i64(_isa: Isa, values: &[i64], sel: &[u32], out: &mut Vec<i64>) {
    portable::gather::gather_i64(values, sel, out);
}

/// `Kernels::hash_i64`: four or eight words a step on AVX2 or AVX-512,
/// a selection's gathered first.
pub(crate) fn hash_i64(isa: Isa, values: &[i64], seed: u64, sel: Option<&[u32]>, out: &mut Vec<u64>) {
    match isa {
        Isa::Portable | Isa::Sse2 => portable::hash::hash_i64(values, seed, sel, out),
        Isa::Avx2 => hash::hash_i64_avx2(values, seed, sel, out),
        Isa::Avx512 => hash::hash_i64_avx512(values, seed, sel, out),
    }
}

/// `Kernels::combine_i64`: four or eight joins a step on AVX2 or AVX-512.
pub(crate) fn combine_i64(isa: Isa, hashes: &mut [u64], next: &[u64], seed: u64) {
    match isa {
        Isa::Portable | Isa::Sse2 => portable::hash::combine_i64(hashes, next, seed),
        Isa::Avx2 => hash::combine_i64_avx2(hashes, next, seed),
        Isa::Avx512 => hash::combine_i64_avx512(hashes, next, seed),
    }
}

/// `Kernels::alp_decode`: four or eight values a step on AVX2 or AVX-512.
pub(crate) fn alp_decode(isa: Isa, words: &mut [i64], scale: AlpScale) {
    match isa {
        Isa::Portable | Isa::Sse2 => portable::alp::alp_decode(words, scale),
        Isa::Avx2 => alp::decode_avx2(words, scale),
        Isa::Avx512 => alp::decode_avx512(words, scale),
    }
}

/// `Kernels::normalize_i64`: the portable body at every set, whose
/// byte swap is one instruction a key and whose stores are each a key's.
pub(crate) fn normalize_i64(
    _isa: Isa,
    given: (&[i64], &[u32]),
    shape: (i64, usize, bool),
    place: (usize, usize),
    keys: &mut [u8],
) -> Option<()> {
    portable::normal::normalize_i64(given, shape, place, keys)
}

/// `Kernels::xxh3_accumulate` at each set.
pub(crate) fn xxh3_accumulate(isa: Isa, acc: &mut [u64; ACC_NB], stripes: &[Stripe], secret: &[u8]) {
    let stripes = keyed(stripes, secret);
    match isa {
        Isa::Portable => Scalar.accumulate(acc, stripes, secret),
        // SAFETY: an `Isa` of SSE2 is made only where the processor has it
        // (`isa`), all a `#[target_feature]` function of it asks; the
        // stripes are those whose keys the secret holds (`keyed`), all the
        // function reads of it.
        Isa::Sse2 => unsafe { xxh3::sse2::xxh3_accumulate(acc, stripes, secret) },
        // SAFETY: as above, of AVX2.
        Isa::Avx2 => unsafe { xxh3::avx2::xxh3_accumulate(acc, stripes, secret) },
        // SAFETY: as above, of AVX-512.
        Isa::Avx512 => unsafe { xxh3::avx512::xxh3_accumulate(acc, stripes, secret) },
    }
}

/// `Kernels::xxh3_scramble` at each set.
pub(crate) fn xxh3_scramble(isa: Isa, acc: &mut [u64; ACC_NB], key: &Stripe) {
    match isa {
        Isa::Portable => Scalar.scramble(acc, key),
        // SAFETY: an `Isa` of SSE2 is made only where the processor has it
        // (`isa`); a scramble reads no more than its key.
        Isa::Sse2 => unsafe { xxh3::sse2::xxh3_scramble(acc, key) },
        // SAFETY: as above, of AVX2.
        Isa::Avx2 => unsafe { xxh3::avx2::xxh3_scramble(acc, key) },
        // SAFETY: as above, of AVX-512.
        Isa::Avx512 => unsafe { xxh3::avx512::xxh3_scramble(acc, key) },
    }
}

/// `Kernels::distance_f32`: sixteen lanes a chunk on AVX2 or AVX-512.
pub(crate) fn distance_f32(isa: Isa, a: &[f32], b: &[f32], term: DistanceTerm) -> f32 {
    let ((ca, ra), (cb, rb)) = (a.as_chunks::<LANES>(), b.as_chunks::<LANES>());
    let (starts, chunks) = ((ca.as_ptr().cast(), cb.as_ptr().cast()), ca.len().min(cb.len()));
    let lanes = match isa {
        Isa::Portable | Isa::Sse2 => return distances::distance_f32(a, b, term),
        // SAFETY: an `Isa` of AVX2 is made only where the processor has AVX2
        // and F16C (`isa`), and each start has `chunks` whole chunks of
        // sixteen `f32`s.
        Isa::Avx2 => unsafe { distance::floats_avx2(term, false, starts, chunks) },
        // SAFETY: as above, of AVX-512 F.
        Isa::Avx512 => unsafe { distance::floats_avx512(term, false, starts, chunks) },
    };
    distances::finish_f32(lanes, (ra, rb), term)
}

/// `Kernels::cosine_f32`, as `distance_f32`.
pub(crate) fn cosine_f32(isa: Isa, a: &[f32], b: &[f32]) -> (f32, f32, f32) {
    let ((ca, ra), (cb, rb)) = (a.as_chunks::<LANES>(), b.as_chunks::<LANES>());
    let (starts, chunks) = ((ca.as_ptr().cast(), cb.as_ptr().cast()), ca.len().min(cb.len()));
    let lanes = match isa {
        Isa::Portable | Isa::Sse2 => return distances::cosine_f32(a, b),
        // SAFETY: as `distance_f32`'s, of AVX2.
        Isa::Avx2 => unsafe { distance::cosine_avx2(starts, chunks) },
        // SAFETY: as `distance_f32`'s, of AVX-512 F.
        Isa::Avx512 => unsafe { distance::cosine_avx512(starts, chunks) },
    };
    distances::finish_cosine(lanes, (ra, rb))
}

/// `Kernels::distance_packed`, as `distance_f32`: FLOAT16 widened by F16C
/// or AVX-512's conversion.
pub(crate) fn distance_packed(isa: Isa, a: &[u8], b: &[u8], shape: (Packed, DistanceTerm)) -> f32 {
    let (format, term) = shape;
    let (step, half) = match format {
        Packed::Single => (SINGLE_STEP, false),
        Packed::Half => (HALF_STEP, true),
    };
    let (ca, cb) = (a.chunks_exact(step), b.chunks_exact(step));
    let (rest, chunks) = ((ca.remainder(), cb.remainder()), ca.len().min(cb.len()));
    let starts = (a.as_ptr(), b.as_ptr());
    let lanes = match isa {
        Isa::Portable | Isa::Sse2 => return distances::distance_packed(a, b, shape),
        // SAFETY: an `Isa` of AVX2 is made only where the processor has AVX2
        // and F16C (`isa`), and each start has `chunks` whole chunks of
        // `step` bytes, sixteen elements of the format.
        Isa::Avx2 => unsafe { distance::floats_avx2(term, half, starts, chunks) },
        // SAFETY: as above, of AVX-512 F.
        Isa::Avx512 => unsafe { distance::floats_avx512(term, half, starts, chunks) },
    };
    distances::finish_packed(lanes, rest, shape)
}

/// `Kernels::distance_i8`, as `distance_f32`, in `i32` lanes.
pub(crate) fn distance_i8(isa: Isa, a: &[u8], b: &[u8], term: DistanceTerm) -> i64 {
    let ((ca, ra), (cb, rb)) = (a.as_chunks::<LANES>(), b.as_chunks::<LANES>());
    let (starts, chunks) = ((ca.as_ptr().cast(), cb.as_ptr().cast()), ca.len().min(cb.len()));
    let lanes = match isa {
        Isa::Portable | Isa::Sse2 => return distances::distance_i8(a, b, term),
        // SAFETY: an `Isa` of AVX2 is made only where the processor has it
        // (`isa`), and each start has `chunks` whole chunks of sixteen bytes.
        Isa::Avx2 => unsafe { distance::i8_avx2(term, starts, chunks) },
        // SAFETY: as above, of AVX-512 F.
        Isa::Avx512 => unsafe { distance::i8_avx512(term, starts, chunks) },
    };
    distances::finish_i8(lanes, (ra, rb), term)
}

/// `Kernels::crc32c`: SSE4.2's `crc32` at AVX2 and AVX-512, whose levels
/// have it (`has_avx2`).
pub(crate) fn crc32c(isa: Isa, register: u32, data: &[u8]) -> u32 {
    match isa {
        Isa::Portable | Isa::Sse2 => portable::crc::crc32c(register, data),
        // SAFETY: an `Isa` of AVX2 or AVX-512 is made only where the
        // processor has SSE4.2 (`has_avx2`), all the function asks.
        Isa::Avx2 | Isa::Avx512 => unsafe { crc32c_sse42(register, data) },
    }
}

/// The low 32 bits of a register, where `crc32` leaves it.
const REGISTER: u64 = 0xFFFF_FFFF;

/// CRC-32C's register after `data`, a word an instruction.
#[target_feature(enable = "sse4.2")]
fn crc32c_sse42(register: u32, data: &[u8]) -> u32 {
    let word = |r: u32, w: u64| u32::try_from(_mm_crc32_u64(u64::from(r), w) & REGISTER).unwrap_or_default();
    crate::crc::crc32c_by(word, register, data)
}

/// `Kernels::find_bytes`: 32 or 64 places a step on AVX2 or AVX-512, each
/// marked by its first and last byte (RULES.md D1119).
pub(crate) fn find_bytes(isa: Isa, haystack: &[u8], needle: &[u8]) -> Option<usize> {
    match isa {
        Isa::Portable | Isa::Sse2 => portable::find::find_bytes(haystack, needle),
        Isa::Avx2 => find::find_avx2(haystack, needle),
        Isa::Avx512 => find::find_avx512(haystack, needle),
    }
}

/// `Kernels::utf8`: 32 bytes a step on AVX2, which AVX-512's level has
/// too (RULES.md D1175).
pub(crate) fn utf8(isa: Isa, bytes: &[u8]) -> Option<&str> {
    match isa {
        Isa::Portable | Isa::Sse2 => std::str::from_utf8(bytes).ok(),
        Isa::Avx2 | Isa::Avx512 => utf8::utf8_avx2(bytes),
    }
}
