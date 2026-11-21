//! \[DESIGN\] Selections on AVX2 and AVX-512 (RULES.md D570): a vector's
//! rows compared four (AVX2, `vpcmpgtq`, `vcmppd`, `vmovmskpd`) or eight
//! (AVX-512, `vpcmpq` and `vcmppd` into a mask) at a time into words of
//! truth bits (`rows.rs`); their places stored eight a byte widened from
//! `rows::offsets` (AVX2), or sixteen a half-word compressed by
//! `vpcompressd` (AVX-512), as Polychroniou et al. (2015) §4 store a
//! selection's places. A function of a set is called only by an `Isa` of
//! it, made only where the processor has it (`x86::isa`).

use std::arch::x86_64::{
    __m256i, __m512i, _mm_loadl_epi64, _mm256_add_epi32, _mm256_castsi256_pd, _mm256_cmp_pd, _mm256_cmpeq_epi64, _mm256_cmpgt_epi64,
    _mm256_cvtepu8_epi32, _mm256_loadu_pd, _mm256_loadu_si256, _mm256_movemask_pd, _mm256_set1_epi32, _mm256_set1_epi64x, _mm256_set1_pd,
    _mm256_storeu_si256, _mm512_add_epi32, _mm512_cmp_epi64_mask, _mm512_cmp_pd_mask, _mm512_loadu_pd, _mm512_loadu_si512,
    _mm512_maskz_compress_epi32, _mm512_set1_epi32, _mm512_set1_epi64, _mm512_set1_pd, _mm512_setr_epi32, _mm512_storeu_si512,
};

use turborel::storage::Mask;
use turborel::value::CompOp;

use super::Isa;
use crate::rows::{self, WORD_BITS, chunk_of, holds_f64, holds_i64, tail_bits, too_long};

/// The operators, as the loops' constants: `=`.
const EQ: u8 = 0;
/// `<>`.
const NE: u8 = 1;
/// `<`.
const LT: u8 = 2;
/// `>`.
const GT: u8 = 3;
/// `<=`.
const LE: u8 = 4;
/// `>=`.
const GE: u8 = 5;

/// Writes the places of the bits set of `words`, from row `base`, to
/// `slots`, by the set's stores.
fn emit(isa: Isa, words: &[u64], base: u32, slots: &mut [u32]) -> usize {
    match isa {
        Isa::Portable | Isa::Sse2 => rows::emit_scalar(words, base, slots),
        // SAFETY: an `Isa` of AVX2 is made only where the processor has it
        // (`x86::isa`); the stores write within `slots`, checked.
        Isa::Avx2 => unsafe { emit_avx2(words, base, slots) },
        // SAFETY: as above, of AVX-512.
        Isa::Avx512 => unsafe { emit_avx512(words, base, slots) },
    }
}

/// `Kernels::select_i64` of all rows, on AVX2.
pub(super) fn i64_avx2(values: &[i64], (op, c): (CompOp, i64), nulls: Option<&Mask>, out: &mut Vec<u32>) {
    if too_long(values.len()) {
        return turborel::kernels::portable::select::select_i64(values, (op, c), (nulls, None), out);
    }
    let truth = |from: usize, words: &mut [u64]| {
        let chunk = chunk_of(values, from, words);
        // SAFETY: an `Isa` of AVX2 is made only where the processor has
        // it (`x86::isa`), and only such a one calls this.
        unsafe { truth_i64_avx2(chunk, (op, c), words) }
    };
    rows::select_rows(values.len(), nulls, out, (truth, |w: &[u64], b, s: &mut [u32]| emit(Isa::Avx2, w, b, s)));
}

/// `Kernels::select_i64` of all rows, on AVX-512.
pub(super) fn i64_avx512(values: &[i64], (op, c): (CompOp, i64), nulls: Option<&Mask>, out: &mut Vec<u32>) {
    if too_long(values.len()) {
        return turborel::kernels::portable::select::select_i64(values, (op, c), (nulls, None), out);
    }
    let truth = |from: usize, words: &mut [u64]| {
        let chunk = chunk_of(values, from, words);
        // SAFETY: an `Isa` of AVX-512 is made only where the processor has
        // it (`x86::isa`), and only such a one calls this.
        unsafe { truth_i64_avx512(chunk, (op, c), words) }
    };
    rows::select_rows(values.len(), nulls, out, (truth, |w: &[u64], b, s: &mut [u32]| emit(Isa::Avx512, w, b, s)));
}

/// `Kernels::select_f64` of all rows, on AVX2.
pub(super) fn f64_avx2(values: &[f64], (op, c): (CompOp, f64), nulls: Option<&Mask>, out: &mut Vec<u32>) {
    if too_long(values.len()) {
        return turborel::kernels::portable::select::select_f64(values, (op, c), (nulls, None), out);
    }
    let truth = |from: usize, words: &mut [u64]| {
        let chunk = chunk_of(values, from, words);
        // SAFETY: as `i64_avx2`'s.
        unsafe { truth_f64_avx2(chunk, (op, c), words) }
    };
    rows::select_rows(values.len(), nulls, out, (truth, |w: &[u64], b, s: &mut [u32]| emit(Isa::Avx2, w, b, s)));
}

/// `Kernels::select_f64` of all rows, on AVX-512.
pub(super) fn f64_avx512(values: &[f64], (op, c): (CompOp, f64), nulls: Option<&Mask>, out: &mut Vec<u32>) {
    if too_long(values.len()) {
        return turborel::kernels::portable::select::select_f64(values, (op, c), (nulls, None), out);
    }
    let truth = |from: usize, words: &mut [u64]| {
        let chunk = chunk_of(values, from, words);
        // SAFETY: as `i64_avx512`'s.
        unsafe { truth_f64_avx512(chunk, (op, c), words) }
    };
    rows::select_rows(values.len(), nulls, out, (truth, |w: &[u64], b, s: &mut [u32]| emit(Isa::Avx512, w, b, s)));
}

/// `Kernels::select_mask` of all rows, by the set's stores.
pub(super) fn mask(isa: Isa, bits: &[u64], nulls: Option<&Mask>, out: &mut Vec<u32>) {
    rows::select_mask(bits, nulls, out, |w: &[u64], b, s: &mut [u32]| emit(isa, w, b, s));
}

/// `Kernels::select_codes` of all rows, by the set's stores.
pub(super) fn codes(isa: Isa, given: (&[u32], &[u64]), nulls: Option<&Mask>, out: &mut Vec<u32>) {
    rows::select_codes(given, nulls, out, |w: &[u64], b, s: &mut [u32]| emit(isa, w, b, s));
}

/// The truth bits of `v op c` of four integers, `OP` one of the operators'
/// constants.
#[target_feature(enable = "avx2")]
fn compare_i64<const OP: u8>(v: __m256i, c: __m256i) -> u64 {
    let (m, inverted) = match OP {
        EQ => (_mm256_cmpeq_epi64(v, c), false),
        NE => (_mm256_cmpeq_epi64(v, c), true),
        LT => (_mm256_cmpgt_epi64(c, v), false),
        GT => (_mm256_cmpgt_epi64(v, c), false),
        LE => (_mm256_cmpgt_epi64(v, c), true),
        _ => (_mm256_cmpgt_epi64(c, v), true),
    };
    let bits = u64::from(_mm256_movemask_pd(_mm256_castsi256_pd(m)).cast_unsigned());
    if inverted { bits ^ 0xF } else { bits }
}

/// The truth bits of `values`, at most a chunk, into `words`, on AVX2.
#[target_feature(enable = "avx2")]
fn truth_i64_avx2(values: &[i64], (op, c): (CompOp, i64), words: &mut [u64]) {
    match op {
        CompOp::Eq => bits_i64_avx2::<EQ>(values, (op, c), words),
        CompOp::Ne => bits_i64_avx2::<NE>(values, (op, c), words),
        CompOp::Lt => bits_i64_avx2::<LT>(values, (op, c), words),
        CompOp::Gt => bits_i64_avx2::<GT>(values, (op, c), words),
        CompOp::Le => bits_i64_avx2::<LE>(values, (op, c), words),
        CompOp::Ge => bits_i64_avx2::<GE>(values, (op, c), words),
    }
}

/// `truth_i64_avx2` of the operator `OP`.
#[target_feature(enable = "avx2")]
fn bits_i64_avx2<const OP: u8>(values: &[i64], (op, c): (CompOp, i64), words: &mut [u64]) {
    let constant = _mm256_set1_epi64x(c);
    for (word, block) in words.iter_mut().zip(values.chunks(WORD_BITS)) {
        let (quads, tail) = block.as_chunks::<4>();
        let mut w = 0_u64;
        for (n, quad) in (0_u32..).zip(quads) {
            // SAFETY: `quad` is 32 bytes, and an unaligned load reads exactly them.
            let v = unsafe { _mm256_loadu_si256(quad.as_ptr().cast()) };
            w |= compare_i64::<OP>(v, constant).wrapping_shl(n.wrapping_mul(4));
        }
        let at = u32::try_from(quads.len()).unwrap_or(0).wrapping_mul(4);
        *word = w | tail_bits(tail, at, |v| holds_i64(op, v, c));
    }
}

/// The truth bits of `values`, at most a chunk, into `words`, on AVX2.
#[target_feature(enable = "avx2")]
fn truth_f64_avx2(values: &[f64], (op, c): (CompOp, f64), words: &mut [u64]) {
    // `_CMP_EQ_OQ`, `_CMP_NEQ_OQ`, `_CMP_LT_OQ`, `_CMP_GT_OQ`,
    // `_CMP_LE_OQ`, `_CMP_GE_OQ`: ordered, so a NaN holds of none.
    match op {
        CompOp::Eq => bits_f64_avx2::<0x00>(values, (op, c), words),
        CompOp::Ne => bits_f64_avx2::<0x0C>(values, (op, c), words),
        CompOp::Lt => bits_f64_avx2::<0x11>(values, (op, c), words),
        CompOp::Gt => bits_f64_avx2::<0x1E>(values, (op, c), words),
        CompOp::Le => bits_f64_avx2::<0x12>(values, (op, c), words),
        CompOp::Ge => bits_f64_avx2::<0x1D>(values, (op, c), words),
    }
}

/// `truth_f64_avx2` of the predicate `P`.
#[target_feature(enable = "avx2")]
fn bits_f64_avx2<const P: i32>(values: &[f64], (op, c): (CompOp, f64), words: &mut [u64]) {
    let constant = _mm256_set1_pd(c);
    for (word, block) in words.iter_mut().zip(values.chunks(WORD_BITS)) {
        let (quads, tail) = block.as_chunks::<4>();
        let mut w = 0_u64;
        for (n, quad) in (0_u32..).zip(quads) {
            // SAFETY: `quad` is 32 bytes, and an unaligned load reads exactly them.
            let v = unsafe { _mm256_loadu_pd(quad.as_ptr()) };
            let bits = u64::from(_mm256_movemask_pd(_mm256_cmp_pd::<P>(v, constant)).cast_unsigned());
            w |= bits.wrapping_shl(n.wrapping_mul(4));
        }
        let at = u32::try_from(quads.len()).unwrap_or(0).wrapping_mul(4);
        *word = w | tail_bits(tail, at, |v| holds_f64(op, v, c));
    }
}

/// The truth bits of `values`, at most a chunk, into `words`, on AVX-512.
#[target_feature(enable = "avx512f")]
fn truth_i64_avx512(values: &[i64], (op, c): (CompOp, i64), words: &mut [u64]) {
    // `_MM_CMPINT_EQ`, `_NE`, `_LT`, `_NLE` (>), `_LE`, `_NLT` (>=).
    match op {
        CompOp::Eq => bits_i64_avx512::<0>(values, (op, c), words),
        CompOp::Ne => bits_i64_avx512::<4>(values, (op, c), words),
        CompOp::Lt => bits_i64_avx512::<1>(values, (op, c), words),
        CompOp::Gt => bits_i64_avx512::<6>(values, (op, c), words),
        CompOp::Le => bits_i64_avx512::<2>(values, (op, c), words),
        CompOp::Ge => bits_i64_avx512::<5>(values, (op, c), words),
    }
}

/// `truth_i64_avx512` of the comparison `P`.
#[target_feature(enable = "avx512f")]
fn bits_i64_avx512<const P: i32>(values: &[i64], (op, c): (CompOp, i64), words: &mut [u64]) {
    let constant = _mm512_set1_epi64(c);
    for (word, block) in words.iter_mut().zip(values.chunks(WORD_BITS)) {
        let (octets, tail) = block.as_chunks::<8>();
        let mut w = 0_u64;
        for (n, octet) in (0_u32..).zip(octets) {
            // SAFETY: `octet` is 64 bytes, and an unaligned load reads exactly them.
            let v = unsafe { _mm512_loadu_si512(octet.as_ptr().cast()) };
            w |= u64::from(_mm512_cmp_epi64_mask::<P>(v, constant)).wrapping_shl(n.wrapping_mul(8));
        }
        let at = u32::try_from(octets.len()).unwrap_or(0).wrapping_mul(8);
        *word = w | tail_bits(tail, at, |v| holds_i64(op, v, c));
    }
}

/// The truth bits of `values`, at most a chunk, into `words`, on AVX-512.
#[target_feature(enable = "avx512f")]
fn truth_f64_avx512(values: &[f64], (op, c): (CompOp, f64), words: &mut [u64]) {
    match op {
        CompOp::Eq => bits_f64_avx512::<0x00>(values, (op, c), words),
        CompOp::Ne => bits_f64_avx512::<0x0C>(values, (op, c), words),
        CompOp::Lt => bits_f64_avx512::<0x11>(values, (op, c), words),
        CompOp::Gt => bits_f64_avx512::<0x1E>(values, (op, c), words),
        CompOp::Le => bits_f64_avx512::<0x12>(values, (op, c), words),
        CompOp::Ge => bits_f64_avx512::<0x1D>(values, (op, c), words),
    }
}

/// `truth_f64_avx512` of the predicate `P`.
#[target_feature(enable = "avx512f")]
fn bits_f64_avx512<const P: i32>(values: &[f64], (op, c): (CompOp, f64), words: &mut [u64]) {
    let constant = _mm512_set1_pd(c);
    for (word, block) in words.iter_mut().zip(values.chunks(WORD_BITS)) {
        let (octets, tail) = block.as_chunks::<8>();
        let mut w = 0_u64;
        for (n, octet) in (0_u32..).zip(octets) {
            // SAFETY: `octet` is 64 bytes, and an unaligned load reads exactly them.
            let v = unsafe { _mm512_loadu_pd(octet.as_ptr()) };
            w |= u64::from(_mm512_cmp_pd_mask::<P>(v, constant)).wrapping_shl(n.wrapping_mul(8));
        }
        let at = u32::try_from(octets.len()).unwrap_or(0).wrapping_mul(8);
        *word = w | tail_bits(tail, at, |v| holds_f64(op, v, c));
    }
}

/// The places of the bits set of `words`, from row `base`, into `slots`,
/// eight a byte, on AVX2: none written past `slots`.
#[target_feature(enable = "avx2,popcnt")]
fn emit_avx2(words: &[u64], base: u32, slots: &mut [u32]) -> usize {
    if slots.len() < words.len().saturating_mul(WORD_BITS).saturating_add(8) {
        return rows::emit_scalar(words, base, slots);
    }
    let table = rows::offsets();
    let mut k = 0_usize;
    for (n, &w) in (0_u32..).zip(words) {
        if w == 0 {
            continue;
        }
        let row = base.wrapping_add(n.wrapping_mul(64));
        for (j, byte) in (0_u32..).zip(w.to_le_bytes()) {
            let (Some(places), Some(slot)) = (table.get(usize::from(byte)), slots.get_mut(k..).and_then(<[u32]>::first_chunk_mut::<8>))
            else {
                break;
            };
            // SAFETY: `places` is 8 bytes, and the load reads exactly them.
            let offsets = unsafe { _mm_loadl_epi64(places.as_ptr().cast()) };
            let at = _mm256_set1_epi32(row.wrapping_add(j.wrapping_mul(8)).cast_signed());
            let rows = _mm256_add_epi32(_mm256_cvtepu8_epi32(offsets), at);
            // SAFETY: `slot` is 8 places, 32 bytes, and an unaligned store
            // writes exactly them.
            unsafe { _mm256_storeu_si256(slot.as_mut_ptr().cast(), rows) };
            k = k.wrapping_add(usize::try_from(byte.count_ones()).unwrap_or(0));
        }
    }
    k
}

/// The places of the bits set of `words`, from row `base`, into `slots`,
/// sixteen a half-word compressed, on AVX-512: none written past `slots`.
#[target_feature(enable = "avx512f,popcnt")]
fn emit_avx512(words: &[u64], base: u32, slots: &mut [u32]) -> usize {
    if slots.len() < words.len().saturating_mul(WORD_BITS).saturating_add(16) {
        return rows::emit_scalar(words, base, slots);
    }
    let iota = _mm512_setr_epi32(0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15);
    let mut k = 0_usize;
    for (n, &w) in (0_u32..).zip(words) {
        if w == 0 {
            continue;
        }
        let row = base.wrapping_add(n.wrapping_mul(64));
        for (j, half) in (0_u32..).zip(w.to_le_bytes().as_chunks::<2>().0) {
            let Some(slot) = slots.get_mut(k..).and_then(<[u32]>::first_chunk_mut::<16>) else { break };
            let bits = u16::from_le_bytes(*half);
            let at: __m512i = _mm512_set1_epi32(row.wrapping_add(j.wrapping_mul(16)).cast_signed());
            let rows = _mm512_maskz_compress_epi32(bits, _mm512_add_epi32(iota, at));
            // SAFETY: `slot` is 16 places, 64 bytes, and an unaligned store
            // writes exactly them.
            unsafe { _mm512_storeu_si512(slot.as_mut_ptr().cast(), rows) };
            k = k.wrapping_add(usize::try_from(bits.count_ones()).unwrap_or(0));
        }
    }
    k
}
