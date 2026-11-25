//! \[DESIGN\] A blocked Bloom filter's probe on AVX2 and AVX-512 (RULES.md
//! D1087), each hash judged exactly as `portable::bloom::may_hold` judges
//! it. The portable probe reads a hash's four bits one after another and
//! stops at the first unset, a branch on each bit that a filter holding
//! few of the probes' keys cannot predict. Here four hashes (AVX2) or
//! eight (AVX-512) have their block picked and their four words and bits
//! made a vector at a time: the block by a multiply of 32 bits of the
//! hash's high 28 and the count of blocks, as `block_of` scales them, the
//! words and bits by shifts of its four 9-bit fields. The words are then
//! read one at a time, as a vector gather measured slower than loads of a
//! word at a time on these processors (`x86.rs`, `gather_i64`), every bit
//! tested with no branch, and each hash's place written and kept only if
//! all four are set. A function of a set is called only by an `Isa` of
//! it, made only where the processor has it (`x86::isa`).

use std::arch::x86_64::{
    __m256i, __m512i, _mm256_add_epi64, _mm256_and_si256, _mm256_loadu_si256, _mm256_mul_epu32, _mm256_set1_epi64x, _mm256_slli_epi64,
    _mm256_sllv_epi64, _mm256_srli_epi64, _mm256_srlv_epi64, _mm256_storeu_si256, _mm512_add_epi64, _mm512_and_si512, _mm512_loadu_si512,
    _mm512_mul_epu32, _mm512_set1_epi64, _mm512_slli_epi64, _mm512_sllv_epi64, _mm512_srli_epi64, _mm512_srlv_epi64, _mm512_storeu_si512,
};

use turborel::kernels::portable::bloom::{self, BLOCK_WORDS};

/// Bits of a hash that pick a bit of a block of 512, as the portable
/// filter's fields are.
const FIELD: u64 = 9;

/// How far a hash is turned to give the bits its block is picked by.
const BLOCK_SHIFT: u32 = 36;

/// How far the product of those bits and the count of blocks is turned
/// to give the block: the 28 bits they are.
const SCALED: u32 = 28;

/// Bits a key sets in its block.
const PROBES: usize = 4;

/// Appends to `out` the places from `from` on among `hashes` that the
/// filter of `words` may hold, in order, by the portable test: those past
/// the last full vector.
fn tail(words: &[u64], hashes: &[u64], from: usize, out: &mut Vec<u32>) {
    let places = (from..).map(|n| u32::try_from(n).unwrap_or(u32::MAX));
    out.extend(hashes.get(from..).unwrap_or_default().iter().zip(places).filter(|&(&h, _)| bloom::may_hold(words, h)).map(|(_, n)| n));
}

/// The count of a filter's blocks, if it has one and a multiply of 32
/// bits scales a hash's bits to it as `block_of` does; of more, which no
/// filter made has, the portable probe judges the hashes.
fn blocks_of(words: &[u64]) -> Option<u64> {
    let blocks = words.as_chunks::<BLOCK_WORDS>().0.len();
    u32::try_from(blocks).ok().filter(|&b| b > 0).map(u64::from)
}

/// How far a hash is turned to give its `n`th field.
fn shift_of(n: usize) -> u64 {
    FIELD.saturating_mul(u64::try_from(n).unwrap_or(0))
}

/// Whether every one of a hash's four bits `bits` is set in the words of
/// `words` at `at`: 1 if so, else 0, by no branch.
fn held(words: &[u64], at: [u64; PROBES], bits: [u64; PROBES]) -> usize {
    let set = |n: usize| {
        let word = at.get(n).and_then(|&w| words.get(usize::try_from(w).unwrap_or(usize::MAX))).copied().unwrap_or(0);
        usize::from(word & bits.get(n).copied().unwrap_or(0) != 0)
    };
    set(0) & set(1) & set(2) & set(3)
}

/// Writes place `n` at the end of `out` and keeps it if `keep` is 1, by
/// no branch on it.
fn kept(out: &mut Vec<u32>, n: usize, keep: usize) {
    let len = out.len();
    out.push(u32::try_from(n).unwrap_or(u32::MAX));
    out.truncate(len.saturating_add(keep));
}

/// `Kernels::bloom_probe` on AVX2.
pub(super) fn probe_avx2(words: &[u64], hashes: &[u64], out: &mut Vec<u32>) {
    let Some(blocks) = blocks_of(words) else { return bloom::bloom_probe(words, hashes, out) };
    out.reserve(hashes.len());
    let (quads, rest) = hashes.as_chunks::<4>();
    // SAFETY: an `Isa` of AVX2 is made only where the processor has it
    // (`x86::isa`), and only such a one calls this.
    unsafe { quads_avx2(words, blocks, quads, out) };
    tail(words, hashes, hashes.len().saturating_sub(rest.len()), out);
}

/// `Kernels::bloom_probe` on AVX-512.
pub(super) fn probe_avx512(words: &[u64], hashes: &[u64], out: &mut Vec<u32>) {
    let Some(blocks) = blocks_of(words) else { return bloom::bloom_probe(words, hashes, out) };
    out.reserve(hashes.len());
    let (octets, rest) = hashes.as_chunks::<8>();
    // SAFETY: as `probe_avx2`'s, of AVX-512.
    unsafe { octets_avx512(words, blocks, octets, out) };
    tail(words, hashes, hashes.len().saturating_sub(rest.len()), out);
}

/// Each 64-bit lane `w`, on AVX2.
#[target_feature(enable = "avx2")]
fn splat256(w: u64) -> __m256i {
    _mm256_set1_epi64x(w.cast_signed())
}

/// Of each hash of `h`, the place among the words of its block's first
/// word, of `count` blocks, on AVX2: `block_of`'s scaling, times 8.
#[target_feature(enable = "avx2")]
fn base_avx2(h: __m256i, count: __m256i) -> __m256i {
    let scaled = _mm256_mul_epu32(_mm256_srli_epi64::<{ BLOCK_SHIFT.cast_signed() }>(h), count);
    _mm256_slli_epi64::<3>(_mm256_srli_epi64::<{ SCALED.cast_signed() }>(scaled))
}

/// The places of the hashes of `quads`, four a step, that the filter of
/// `words`, of `blocks` blocks, may hold, appended to `out`.
#[target_feature(enable = "avx2")]
fn quads_avx2(words: &[u64], blocks: u64, quads: &[[u64; 4]], out: &mut Vec<u32>) {
    let (count, field, low, one) = (splat256(blocks), splat256((1 << FIELD) - 1), splat256(63), splat256(1));
    let (mut at, mut bits) = ([[0_u64; 4]; PROBES], [[0_u64; 4]; PROBES]);
    for (q, quad) in quads.iter().enumerate() {
        // SAFETY: `quad` is 32 bytes, and an unaligned load reads exactly them.
        let h = unsafe { _mm256_loadu_si256(quad.as_ptr().cast()) };
        let base = base_avx2(h, count);
        for (n, (word_at, bit_of)) in at.iter_mut().zip(bits.iter_mut()).enumerate() {
            let f = _mm256_and_si256(_mm256_srlv_epi64(h, splat256(shift_of(n))), field);
            let word = _mm256_add_epi64(base, _mm256_srli_epi64::<6>(f));
            let bit = _mm256_sllv_epi64(one, _mm256_and_si256(f, low));
            // SAFETY: `word_at` is 32 bytes, and an unaligned store writes
            // exactly them.
            unsafe { _mm256_storeu_si256(word_at.as_mut_ptr().cast(), word) };
            // SAFETY: as of `word_at`, of `bit_of`.
            unsafe { _mm256_storeu_si256(bit_of.as_mut_ptr().cast(), bit) };
        }
        for lane in 0..4 {
            let pick = |of: &[[u64; 4]; PROBES]| of.map(|v| v.get(lane).copied().unwrap_or(0));
            kept(out, q.saturating_mul(4).saturating_add(lane), held(words, pick(&at), pick(&bits)));
        }
    }
}

/// Each 64-bit lane `w`, on AVX-512.
#[target_feature(enable = "avx512f")]
fn splat512(w: u64) -> __m512i {
    _mm512_set1_epi64(w.cast_signed())
}

/// Of each hash of `h`, the place among the words of its block's first
/// word, of `count` blocks, on AVX-512: `block_of`'s scaling, times 8.
#[target_feature(enable = "avx512f")]
fn base_avx512(h: __m512i, count: __m512i) -> __m512i {
    let scaled = _mm512_mul_epu32(_mm512_srli_epi64::<BLOCK_SHIFT>(h), count);
    _mm512_slli_epi64::<3>(_mm512_srli_epi64::<SCALED>(scaled))
}

/// The places of the hashes of `octets`, eight a step, that the filter
/// of `words`, of `blocks` blocks, may hold, appended to `out`.
#[target_feature(enable = "avx512f")]
fn octets_avx512(words: &[u64], blocks: u64, octets: &[[u64; 8]], out: &mut Vec<u32>) {
    let (count, field, low, one) = (splat512(blocks), splat512((1 << FIELD) - 1), splat512(63), splat512(1));
    let (mut at, mut bits) = ([[0_u64; 8]; PROBES], [[0_u64; 8]; PROBES]);
    for (o, octet) in octets.iter().enumerate() {
        // SAFETY: `octet` is 64 bytes, and an unaligned load reads exactly them.
        let h = unsafe { _mm512_loadu_si512(octet.as_ptr().cast()) };
        let base = base_avx512(h, count);
        for (n, (word_at, bit_of)) in at.iter_mut().zip(bits.iter_mut()).enumerate() {
            let f = _mm512_and_si512(_mm512_srlv_epi64(h, splat512(shift_of(n))), field);
            let word = _mm512_add_epi64(base, _mm512_srli_epi64::<6>(f));
            let bit = _mm512_sllv_epi64(one, _mm512_and_si512(f, low));
            // SAFETY: `word_at` is 64 bytes, and an unaligned store writes
            // exactly them.
            unsafe { _mm512_storeu_si512(word_at.as_mut_ptr().cast(), word) };
            // SAFETY: as of `word_at`, of `bit_of`.
            unsafe { _mm512_storeu_si512(bit_of.as_mut_ptr().cast(), bit) };
        }
        for lane in 0..8 {
            let pick = |of: &[[u64; 8]; PROBES]| of.map(|v| v.get(lane).copied().unwrap_or(0));
            kept(out, o.saturating_mul(8).saturating_add(lane), held(words, pick(&at), pick(&bits)));
        }
    }
}
