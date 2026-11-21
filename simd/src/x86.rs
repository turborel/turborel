//! \[DESIGN\] Kernels on x86-64 (RULES.md D570): the instruction sets a
//! `Simd` may hold, each made only of a level the processor has, and each
//! kernel's choice among its bodies by the set. SSE2 is every x86-64
//! processor's, and the compiler already builds the portable bodies for
//! it, so a kernel no SSE2 instruction speeds runs its portable body
//! there; AVX2 and AVX-512 have their own bodies (`select.rs`, `sum.rs`,
//! `alp.rs`), XXH3's loops theirs at each set (`xxh3.rs`).

use turborel::hash::xxhash::{ACC_NB, Scalar, Stripe, Stripes as _, keyed};
use turborel::kernels::{AlpScale, Given, portable};
use turborel::value::CompOp;

use crate::Level;

mod alp;
mod hash;
mod select;
mod sum;
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

/// `Kernels::select_codes`, as `select_i64`.
pub(crate) fn select_codes(isa: Isa, codes: &[u32], keep: &[u64], given: Given<'_>, out: &mut Vec<u32>) {
    let (nulls, None) = given else { return portable::select::select_codes(codes, keep, given, out) };
    match isa {
        Isa::Portable | Isa::Sse2 => portable::select::select_codes(codes, keep, given, out),
        Isa::Avx2 | Isa::Avx512 => select::codes(isa, (codes, keep), nulls, out),
    }
}

/// `Kernels::sum_i64`: of all rows on AVX2 or AVX-512, of a selection by
/// the portable body.
pub(crate) fn sum_i64(isa: Isa, values: &[i64], sel: Option<&[u32]>) -> i128 {
    let None = sel else { return portable::sum::sum_i64(values, sel) };
    match isa {
        Isa::Portable | Isa::Sse2 => portable::sum::sum_i64(values, sel),
        Isa::Avx2 => sum::sum_avx2(values),
        Isa::Avx512 => sum::sum_avx512(values),
    }
}

/// `Kernels::min_max_i64`, as `sum_i64`.
pub(crate) fn min_max_i64(isa: Isa, values: &[i64], given: Given<'_>) -> Option<(i64, i64)> {
    let (nulls, None) = given else { return portable::sum::min_max_i64(values, given) };
    match isa {
        Isa::Portable | Isa::Sse2 => portable::sum::min_max_i64(values, given),
        Isa::Avx2 => sum::min_max_avx2(values, nulls),
        Isa::Avx512 => sum::min_max_avx512(values, nulls),
    }
}

/// `Kernels::gather_i64`: by the vector gathers of AVX2 or AVX-512.
pub(crate) fn gather_i64(isa: Isa, values: &[i64], sel: &[u32], out: &mut Vec<i64>) {
    match isa {
        Isa::Portable | Isa::Sse2 => portable::gather::gather_i64(values, sel, out),
        Isa::Avx2 => sum::gather_avx2(values, sel, out),
        Isa::Avx512 => sum::gather_avx512(values, sel, out),
    }
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
