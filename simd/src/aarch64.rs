//! \[DESIGN\] Kernels on AArch64 (RULES.md D570): NEON, which every such
//! processor has, a module of `#[target_feature]` functions for each
//! kind of kernel (`neon.rs` for selections, sums and decimals, `xxh3.rs`
//! for XXH3's loops, `distance.rs` for distance sums), CRC-32C on the CRC
//! extension's instructions where the processor has them (`crate::crc`),
//! and each kernel's choice among its bodies. NEON has
//! no gather and no wider sum of a selection than the portable loop, so
//! those run their portable bodies.

use std::arch::aarch64::__crc32cd;
use turborel::hash::xxhash::{ACC_NB, Scalar, Stripe, Stripes as _, keyed};

use turborel::kernels::portable::distance::{self as distances, LANES, SINGLE_STEP};
use turborel::kernels::{AlpScale, DistanceTerm, Given, Packed, portable};
use turborel::value::CompOp;

use crate::Level;

mod distance;
mod neon;
mod xxh3;

/// \[DESIGN\] The instructions a `Simd` runs on: made only by `isa`, of a
/// level the processor has, so a `#[target_feature]` function of its set
/// called on it runs on instructions the processor has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Isa {
    /// The portable bodies.
    Portable,
    /// NEON.
    Neon,
    /// NEON, and the CRC extension's instructions.
    NeonCrc,
}

/// The portable set, which needs no instruction detected.
pub(crate) const PORTABLE: Isa = Isa::Portable;

/// The instructions of `level`, if the processor has them: the only maker
/// of an `Isa` but the portable one.
pub(crate) fn isa(level: Level) -> Option<Isa> {
    match level {
        Level::Portable => Some(Isa::Portable),
        Level::Neon => std::arch::is_aarch64_feature_detected!("neon")
            .then(|| if std::arch::is_aarch64_feature_detected!("crc") { Isa::NeonCrc } else { Isa::Neon }),
        Level::Sse2 | Level::Avx2 | Level::Avx512 => None,
    }
}

/// `Kernels::select_i64`: of all rows on NEON, of a selection by the
/// portable body.
pub(crate) fn select_i64(isa: Isa, values: &[i64], pred: (CompOp, i64), given: Given<'_>, out: &mut Vec<u32>) {
    let (nulls, None) = given else { return portable::select::select_i64(values, pred, given, out) };
    match isa {
        Isa::Portable => portable::select::select_i64(values, pred, given, out),
        Isa::Neon | Isa::NeonCrc => neon::select_i64(values, pred, nulls, out),
    }
}

/// `Kernels::select_f64`, as `select_i64`.
pub(crate) fn select_f64(isa: Isa, values: &[f64], pred: (CompOp, f64), given: Given<'_>, out: &mut Vec<u32>) {
    let (nulls, None) = given else { return portable::select::select_f64(values, pred, given, out) };
    match isa {
        Isa::Portable => portable::select::select_f64(values, pred, given, out),
        Isa::Neon | Isa::NeonCrc => neon::select_f64(values, pred, nulls, out),
    }
}

/// `Kernels::select_mask`, as `select_i64`.
pub(crate) fn select_mask(isa: Isa, bits: &[u64], given: Given<'_>, out: &mut Vec<u32>) {
    let (nulls, None) = given else { return portable::select::select_mask(bits, given, out) };
    match isa {
        Isa::Portable => portable::select::select_mask(bits, given, out),
        Isa::Neon | Isa::NeonCrc => neon::select_mask(bits, nulls, out),
    }
}

/// `Kernels::select_codes`, as `select_i64`.
pub(crate) fn select_codes(isa: Isa, codes: &[u32], keep: &[u64], given: Given<'_>, out: &mut Vec<u32>) {
    let (nulls, None) = given else { return portable::select::select_codes(codes, keep, given, out) };
    match isa {
        Isa::Portable => portable::select::select_codes(codes, keep, given, out),
        Isa::Neon | Isa::NeonCrc => neon::select_codes((codes, keep), nulls, out),
    }
}

/// `Kernels::sum_i64`: of all rows on NEON, of a selection by the
/// portable body.
pub(crate) fn sum_i64(isa: Isa, values: &[i64], sel: Option<&[u32]>) -> i128 {
    let None = sel else { return portable::sum::sum_i64(values, sel) };
    match isa {
        Isa::Portable => portable::sum::sum_i64(values, sel),
        Isa::Neon | Isa::NeonCrc => neon::sum_i64(values),
    }
}

/// `Kernels::min_max_i64`, as `sum_i64`.
pub(crate) fn min_max_i64(isa: Isa, values: &[i64], given: Given<'_>) -> Option<(i64, i64)> {
    let (nulls, None) = given else { return portable::sum::min_max_i64(values, given) };
    match isa {
        Isa::Portable => portable::sum::min_max_i64(values, given),
        Isa::Neon | Isa::NeonCrc => neon::min_max_i64(values, nulls),
    }
}

/// `Kernels::bloom_probe`: the portable body.
pub(crate) fn bloom_probe(_isa: Isa, words: &[u64], hashes: &[u64], out: &mut Vec<u32>) {
    portable::bloom::bloom_probe(words, hashes, out);
}

/// `Kernels::gather_i64`: the portable body, NEON having no gather.
pub(crate) fn gather_i64(_isa: Isa, values: &[i64], sel: &[u32], out: &mut Vec<i64>) {
    portable::gather::gather_i64(values, sel, out);
}

/// `Kernels::hash_i64`: the portable body, whose 64-bit multiplies are
/// each one instruction a word, where a vector's are three of 32 bits a
/// lane, two lanes a step.
pub(crate) fn hash_i64(_isa: Isa, values: &[i64], seed: u64, sel: Option<&[u32]>, out: &mut Vec<u64>) {
    portable::hash::hash_i64(values, seed, sel, out);
}

/// `Kernels::combine_i64`: the portable body, as `hash_i64`, its 128-bit
/// products each two instructions a join.
pub(crate) fn combine_i64(_isa: Isa, hashes: &mut [u64], next: &[u64], seed: u64) {
    portable::hash::combine_i64(hashes, next, seed);
}

/// `Kernels::alp_decode`: two values a step on NEON.
pub(crate) fn alp_decode(isa: Isa, words: &mut [i64], scale: AlpScale) {
    match isa {
        Isa::Portable => portable::alp::alp_decode(words, scale),
        Isa::Neon | Isa::NeonCrc => neon::alp_decode(words, scale),
    }
}

/// `Kernels::normalize_i64`: the portable body, whose byte swap is one
/// instruction a key and whose stores are each a key's.
pub(crate) fn normalize_i64(
    _isa: Isa,
    given: (&[i64], &[u32]),
    shape: (i64, usize, bool),
    place: (usize, usize),
    keys: &mut [u8],
) -> Option<()> {
    portable::normal::normalize_i64(given, shape, place, keys)
}

/// `Kernels::xxh3_accumulate` on NEON.
pub(crate) fn xxh3_accumulate(isa: Isa, acc: &mut [u64; ACC_NB], stripes: &[Stripe], secret: &[u8]) {
    let stripes = keyed(stripes, secret);
    match isa {
        Isa::Portable => Scalar.accumulate(acc, stripes, secret),
        // SAFETY: an `Isa` of NEON, with the CRC extension or without, is
        // made only where the processor has it (`isa`), all the function
        // asks.
        Isa::Neon | Isa::NeonCrc => unsafe { xxh3::xxh3_accumulate(acc, stripes, secret) },
    }
}

/// `Kernels::xxh3_scramble` on NEON.
pub(crate) fn xxh3_scramble(isa: Isa, acc: &mut [u64; ACC_NB], key: &Stripe) {
    match isa {
        Isa::Portable => Scalar.scramble(acc, key),
        // SAFETY: as above.
        Isa::Neon | Isa::NeonCrc => unsafe { xxh3::xxh3_scramble(acc, key) },
    }
}

/// `Kernels::distance_f32`: sixteen lanes a chunk, four registers, on NEON.
pub(crate) fn distance_f32(isa: Isa, a: &[f32], b: &[f32], term: DistanceTerm) -> f32 {
    let ((ca, ra), (cb, rb)) = (a.as_chunks::<LANES>(), b.as_chunks::<LANES>());
    let (starts, chunks) = ((ca.as_ptr().cast(), cb.as_ptr().cast()), ca.len().min(cb.len()));
    let lanes = match isa {
        Isa::Portable => return distances::distance_f32(a, b, term),
        // SAFETY: an `Isa` of NEON is made only where the processor has it
        // (`isa`), and each start has `chunks` whole chunks of sixteen `f32`s.
        Isa::Neon | Isa::NeonCrc => unsafe { distance::floats_neon(term, starts, chunks) },
    };
    distances::finish_f32(lanes, (ra, rb), term)
}

/// `Kernels::cosine_f32`, as `distance_f32`.
pub(crate) fn cosine_f32(isa: Isa, a: &[f32], b: &[f32]) -> (f32, f32, f32) {
    let ((ca, ra), (cb, rb)) = (a.as_chunks::<LANES>(), b.as_chunks::<LANES>());
    let (starts, chunks) = ((ca.as_ptr().cast(), cb.as_ptr().cast()), ca.len().min(cb.len()));
    let lanes = match isa {
        Isa::Portable => return distances::cosine_f32(a, b),
        // SAFETY: as `distance_f32`'s.
        Isa::Neon | Isa::NeonCrc => unsafe { distance::cosine_neon(starts, chunks) },
    };
    distances::finish_cosine(lanes, (ra, rb))
}

/// `Kernels::distance_packed`: FLOAT32 as `distance_f32`, its chunks read
/// as the `f32`s they hold; FLOAT16 by the portable body.
pub(crate) fn distance_packed(isa: Isa, a: &[u8], b: &[u8], shape: (Packed, DistanceTerm)) -> f32 {
    let (Packed::Single, term) = shape else { return distances::distance_packed(a, b, shape) };
    let ((ca, ra), (cb, rb)) = (a.as_chunks::<SINGLE_STEP>(), b.as_chunks::<SINGLE_STEP>());
    let (rest, chunks) = ((ra, rb), ca.len().min(cb.len()));
    let lanes = match isa {
        Isa::Portable => return distances::distance_packed(a, b, shape),
        // SAFETY: an `Isa` of NEON is made only where the processor has it
        // (`isa`), and each start has `chunks` whole chunks of sixteen
        // little-endian `f32`s, the processor's order, read at any alignment.
        Isa::Neon | Isa::NeonCrc => unsafe { distance::floats_neon(term, (a.as_ptr(), b.as_ptr()), chunks) },
    };
    distances::finish_packed(lanes, rest, shape)
}

/// `Kernels::distance_i8`, as `distance_f32`, in `i32` lanes.
pub(crate) fn distance_i8(isa: Isa, a: &[u8], b: &[u8], term: DistanceTerm) -> i64 {
    let ((ca, ra), (cb, rb)) = (a.as_chunks::<LANES>(), b.as_chunks::<LANES>());
    let (starts, chunks) = ((ca.as_ptr().cast(), cb.as_ptr().cast()), ca.len().min(cb.len()));
    let lanes = match isa {
        Isa::Portable => return distances::distance_i8(a, b, term),
        // SAFETY: an `Isa` of NEON is made only where the processor has it
        // (`isa`), and each start has `chunks` whole chunks of sixteen bytes.
        Isa::Neon | Isa::NeonCrc => unsafe { distance::i8_neon(term, starts, chunks) },
    };
    distances::finish_i8(lanes, (ra, rb), term)
}

/// `Kernels::crc32c`: the CRC extension's `crc32cx` where the processor
/// has it, else the portable body.
pub(crate) fn crc32c(isa: Isa, register: u32, data: &[u8]) -> u32 {
    match isa {
        Isa::Portable | Isa::Neon => portable::crc::crc32c(register, data),
        // SAFETY: an `Isa` of NEON with CRC is made only where the processor
        // has the CRC extension (`isa`), all the function asks.
        Isa::NeonCrc => unsafe { crc32c_crc(register, data) },
    }
}

/// CRC-32C's register after `data`, a word an instruction.
#[target_feature(enable = "crc")]
fn crc32c_crc(register: u32, data: &[u8]) -> u32 {
    crate::crc::crc32c_by(|r: u32, w: u64| __crc32cd(r, w), register, data)
}

/// `Kernels::find_bytes`: by the portable body, eight places a step, as
/// NEON has no instruction that gathers a compare's lanes into a mask of
/// bits as x86's do (RULES.md D1121).
pub(crate) fn find_bytes(_isa: Isa, haystack: &[u8], needle: &[u8]) -> Option<usize> {
    portable::find::find_bytes(haystack, needle)
}
