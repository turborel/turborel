//! \[DESIGN\] Hashes of words and joins of a key's hashes on AVX2 and
//! AVX-512 (RULES.md D573), each lane's exactly XXH3-64's as
//! `hash::xxhash::Words` makes it. A word's hash is `XXH3_rrmxmx` of it
//! turned by half and flipped: two 64-bit multiplies, one instruction on
//! AVX-512 DQ, three of 32 bits on AVX2. A join is XXH3-64 of 16 bytes,
//! whose 128-bit product is four multiplies of 32 bits and its halves
//! joined. The integers are gathered as words a block at a time, then
//! hashed in place while the block is in the first cache; the few past the last full vector by `Words`. A
//! function of a set is called only by an `Isa` of it, made only where the
//! processor has it (`x86::isa`).

use std::arch::x86_64::{
    __m256i, __m512i, _mm_setr_epi8, _mm256_add_epi64, _mm256_and_si256, _mm256_loadu_si256, _mm256_mul_epu32, _mm256_or_si256,
    _mm256_set1_epi64x, _mm256_setr_epi8, _mm256_shuffle_epi8, _mm256_shuffle_epi32, _mm256_slli_epi64, _mm256_srli_epi64,
    _mm256_storeu_si256, _mm256_xor_si256, _mm512_add_epi64, _mm512_and_si512, _mm512_broadcast_i32x4, _mm512_loadu_si512,
    _mm512_mul_epu32, _mm512_mullo_epi64, _mm512_or_si512, _mm512_rol_epi64, _mm512_set1_epi64, _mm512_shuffle_epi8, _mm512_slli_epi64,
    _mm512_srli_epi64, _mm512_storeu_si512, _mm512_xor_si512,
};

use turborel::hash::xxhash::{PRIME_MX1, PRIME_MX2, Words};
use turborel::storage::mask::index;

/// The low half of a word.
const LOW: u64 = 0xFFFF_FFFF;

/// Bytes of a word hashed.
const WORD_LEN: u64 = 8;

/// Bytes of two words joined.
const PAIR_LEN: u64 = 16;

/// \[DESIGN\] Rows gathered, then hashed, a block at a time, so a block
/// is hashed while it is in the first cache.
const BLOCK: usize = 2048;

/// Appends to `out` the hash of each of `values` at `sel`, or all, 0 for
/// a row past the end, each block gathered as words and then hashed in
/// place by `hash`.
fn blocks(values: &[i64], sel: Option<&[u32]>, out: &mut Vec<u64>, mut hash: impl FnMut(&mut [u64])) {
    out.reserve(sel.map_or(values.len(), <[u32]>::len));
    let mut hashed = |grown: &mut Vec<u64>, start: usize| hash(grown.get_mut(start..).unwrap_or_default());
    match sel {
        Some(sel) => sel.chunks(BLOCK).for_each(|block| {
            let start = out.len();
            out.extend(block.iter().map(|&r| values.get(index(r)).copied().unwrap_or(0).cast_unsigned()));
            hashed(out, start);
        }),
        None => values.chunks(BLOCK).for_each(|block| {
            let start = out.len();
            out.extend(block.iter().map(|&v| v.cast_unsigned()));
            hashed(out, start);
        }),
    }
}

/// `Kernels::hash_i64` on AVX2.
pub(super) fn hash_i64_avx2(values: &[i64], seed: u64, sel: Option<&[u32]>, out: &mut Vec<u64>) {
    let words = Words::new(seed);
    blocks(values, sel, out, |fresh| {
        // SAFETY: an `Isa` of AVX2 is made only where the processor has
        // it (`x86::isa`), and only such a one calls this.
        unsafe { words_avx2(fresh, &words) }
    });
}

/// `Kernels::hash_i64` on AVX-512.
pub(super) fn hash_i64_avx512(values: &[i64], seed: u64, sel: Option<&[u32]>, out: &mut Vec<u64>) {
    let words = Words::new(seed);
    blocks(values, sel, out, |fresh| {
        // SAFETY: an `Isa` of AVX-512 is made only where the processor has
        // it (`x86::isa`), and only such a one calls this.
        unsafe { words_avx512(fresh, &words) }
    });
}

/// `Kernels::combine_i64` on AVX2.
pub(super) fn combine_i64_avx2(hashes: &mut [u64], next: &[u64], seed: u64) {
    let words = Words::new(seed);
    let both = hashes.len().min(next.len());
    let (hashes, next) = (hashes.get_mut(..both).unwrap_or_default(), next.get(..both).unwrap_or_default());
    // SAFETY: as `hash_i64_avx2`'s.
    unsafe { pairs_avx2(hashes, next, &words) };
}

/// `Kernels::combine_i64` on AVX-512.
pub(super) fn combine_i64_avx512(hashes: &mut [u64], next: &[u64], seed: u64) {
    let words = Words::new(seed);
    let both = hashes.len().min(next.len());
    let (hashes, next) = (hashes.get_mut(..both).unwrap_or_default(), next.get(..both).unwrap_or_default());
    // SAFETY: as `hash_i64_avx512`'s.
    unsafe { pairs_avx512(hashes, next, &words) };
}

/// Each 64-bit lane `w`, on AVX2.
#[target_feature(enable = "avx2")]
fn splat256(w: u64) -> __m256i {
    _mm256_set1_epi64x(w.cast_signed())
}

/// Each 64-bit lane `w`, on AVX-512.
#[target_feature(enable = "avx512f")]
fn splat512(w: u64) -> __m512i {
    _mm512_set1_epi64(w.cast_signed())
}

/// The low 64 bits of each lane of `a` times `(m, m_high)`, `m`'s word and
/// its high half: three multiplies of 32 bits.
#[target_feature(enable = "avx2")]
fn mul_avx2(a: __m256i, (m, m_high): (__m256i, __m256i)) -> __m256i {
    let low = _mm256_mul_epu32(a, m);
    let crossed = _mm256_add_epi64(_mm256_mul_epu32(_mm256_srli_epi64::<32>(a), m), _mm256_mul_epu32(a, m_high));
    _mm256_add_epi64(low, _mm256_slli_epi64::<32>(crossed))
}

/// Each lane of `a` turned left by `N` bits, `M` being 64 less `N`.
#[target_feature(enable = "avx2")]
fn rotl_avx2<const N: i32, const M: i32>(a: __m256i) -> __m256i {
    _mm256_or_si256(_mm256_slli_epi64::<N>(a), _mm256_srli_epi64::<M>(a))
}

/// Each of `words` hashed in place by `hashed`'s flip, on AVX2.
#[target_feature(enable = "avx2")]
fn words_avx2(words: &mut [u64], hashed: &Words) {
    let (flip, len) = (splat256(hashed.bitflip()), splat256(WORD_LEN));
    let (m, m_high) = (splat256(PRIME_MX2), splat256(PRIME_MX2 >> 32_u32));
    let (quads, rest) = words.as_chunks_mut::<4>();
    for quad in quads {
        // SAFETY: `quad` is 32 bytes, and an unaligned load reads exactly them.
        let w = unsafe { _mm256_loadu_si256(quad.as_ptr().cast()) };
        let mut h = _mm256_xor_si256(_mm256_shuffle_epi32::<0xB1>(w), flip);
        h = _mm256_xor_si256(h, _mm256_xor_si256(rotl_avx2::<49, 15>(h), rotl_avx2::<24, 40>(h)));
        h = mul_avx2(h, (m, m_high));
        h = _mm256_xor_si256(h, _mm256_add_epi64(_mm256_srli_epi64::<35>(h), len));
        h = mul_avx2(h, (m, m_high));
        h = _mm256_xor_si256(h, _mm256_srli_epi64::<28>(h));
        // SAFETY: `quad` is 32 bytes, and an unaligned store writes exactly them.
        unsafe { _mm256_storeu_si256(quad.as_mut_ptr().cast(), h) };
    }
    for w in rest {
        *w = hashed.hash(*w);
    }
}

/// Each of `words` hashed in place by `hashed`'s flip, on AVX-512.
#[target_feature(enable = "avx512f,avx512dq")]
fn words_avx512(words: &mut [u64], hashed: &Words) {
    let (flip, m, len) = (splat512(hashed.bitflip()), splat512(PRIME_MX2), splat512(WORD_LEN));
    let (octets, rest) = words.as_chunks_mut::<8>();
    for octet in octets {
        // SAFETY: `octet` is 64 bytes, and an unaligned load reads exactly them.
        let w = unsafe { _mm512_loadu_si512(octet.as_ptr().cast()) };
        let mut h = _mm512_xor_si512(_mm512_rol_epi64::<32>(w), flip);
        h = _mm512_xor_si512(h, _mm512_xor_si512(_mm512_rol_epi64::<49>(h), _mm512_rol_epi64::<24>(h)));
        h = _mm512_mullo_epi64(h, m);
        h = _mm512_xor_si512(h, _mm512_add_epi64(_mm512_srli_epi64::<35>(h), len));
        h = _mm512_mullo_epi64(h, m);
        h = _mm512_xor_si512(h, _mm512_srli_epi64::<28>(h));
        // SAFETY: `octet` is 64 bytes, and an unaligned store writes exactly them.
        unsafe { _mm512_storeu_si512(octet.as_mut_ptr().cast(), h) };
    }
    for w in rest {
        *w = hashed.hash(*w);
    }
}

/// `XXH3_mul128_fold64` of each lane of `a` and `b`, on AVX2: the 128-bit
/// product of four of 32 bits, its halves xored.
#[target_feature(enable = "avx2")]
fn fold_avx2(a: __m256i, b: __m256i) -> __m256i {
    let low_half = splat256(LOW);
    let (a_high, b_high) = (_mm256_srli_epi64::<32>(a), _mm256_srli_epi64::<32>(b));
    let (ll, lh) = (_mm256_mul_epu32(a, b), _mm256_mul_epu32(a, b_high));
    let (hl, hh) = (_mm256_mul_epu32(a_high, b), _mm256_mul_epu32(a_high, b_high));
    let cross = _mm256_add_epi64(_mm256_add_epi64(_mm256_srli_epi64::<32>(ll), _mm256_and_si256(lh, low_half)), hl);
    let high = _mm256_add_epi64(_mm256_add_epi64(hh, _mm256_srli_epi64::<32>(lh)), _mm256_srli_epi64::<32>(cross));
    let low = _mm256_or_si256(_mm256_slli_epi64::<32>(cross), _mm256_and_si256(ll, low_half));
    _mm256_xor_si256(low, high)
}

/// Each of `hashes` joined in place with `next` at its place, on AVX2.
#[target_feature(enable = "avx2")]
fn pairs_avx2(hashes: &mut [u64], next: &[u64], pairs: &Words) {
    let (flips, swap) = (
        pairs.pair_flips(),
        _mm256_setr_epi8(7, 6, 5, 4, 3, 2, 1, 0, 15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0, 15, 14, 13, 12, 11, 10, 9, 8),
    );
    let (first_flip, second_flip, len) = (splat256(flips.0), splat256(flips.1), splat256(PAIR_LEN));
    let (m, m_high) = (splat256(PRIME_MX1), splat256(PRIME_MX1 >> 32_u32));
    let (quads, rest) = hashes.as_chunks_mut::<4>();
    let (next_quads, next_rest) = next.as_chunks::<4>();
    for (quad, next_quad) in quads.iter_mut().zip(next_quads) {
        // SAFETY: `quad` and `next_quad` are 32 bytes each, and an
        // unaligned load reads exactly them.
        let low = _mm256_xor_si256(unsafe { _mm256_loadu_si256(quad.as_ptr().cast()) }, first_flip);
        // SAFETY: as above.
        let high = _mm256_xor_si256(unsafe { _mm256_loadu_si256(next_quad.as_ptr().cast()) }, second_flip);
        let summed = _mm256_add_epi64(_mm256_add_epi64(len, _mm256_shuffle_epi8(low, swap)), high);
        let mut h = _mm256_add_epi64(summed, fold_avx2(low, high));
        h = _mm256_xor_si256(h, _mm256_srli_epi64::<37>(h));
        h = mul_avx2(h, (m, m_high));
        h = _mm256_xor_si256(h, _mm256_srli_epi64::<32>(h));
        // SAFETY: `quad` is 32 bytes, and an unaligned store writes exactly them.
        unsafe { _mm256_storeu_si256(quad.as_mut_ptr().cast(), h) };
    }
    rest.iter_mut().zip(next_rest).for_each(|(h, &n)| *h = pairs.pair(*h, n));
}

/// `XXH3_mul128_fold64` of each lane of `a` and `b`, on AVX-512, as
/// `fold_avx2`.
#[target_feature(enable = "avx512f")]
fn fold_avx512(a: __m512i, b: __m512i) -> __m512i {
    let low_half = splat512(LOW);
    let (a_high, b_high) = (_mm512_srli_epi64::<32>(a), _mm512_srli_epi64::<32>(b));
    let (ll, lh) = (_mm512_mul_epu32(a, b), _mm512_mul_epu32(a, b_high));
    let (hl, hh) = (_mm512_mul_epu32(a_high, b), _mm512_mul_epu32(a_high, b_high));
    let cross = _mm512_add_epi64(_mm512_add_epi64(_mm512_srli_epi64::<32>(ll), _mm512_and_si512(lh, low_half)), hl);
    let high = _mm512_add_epi64(_mm512_add_epi64(hh, _mm512_srli_epi64::<32>(lh)), _mm512_srli_epi64::<32>(cross));
    let low = _mm512_or_si512(_mm512_slli_epi64::<32>(cross), _mm512_and_si512(ll, low_half));
    _mm512_xor_si512(low, high)
}

/// Each of `hashes` joined in place with `next` at its place, on AVX-512.
#[target_feature(enable = "avx512f,avx512bw,avx512dq")]
fn pairs_avx512(hashes: &mut [u64], next: &[u64], pairs: &Words) {
    let flips = pairs.pair_flips();
    let swap = _mm512_broadcast_i32x4(_mm_setr_epi8(7, 6, 5, 4, 3, 2, 1, 0, 15, 14, 13, 12, 11, 10, 9, 8));
    let (first_flip, second_flip, len, m) = (splat512(flips.0), splat512(flips.1), splat512(PAIR_LEN), splat512(PRIME_MX1));
    let (octets, rest) = hashes.as_chunks_mut::<8>();
    let (next_octets, next_rest) = next.as_chunks::<8>();
    for (octet, next_octet) in octets.iter_mut().zip(next_octets) {
        // SAFETY: `octet` and `next_octet` are 64 bytes each, and an
        // unaligned load reads exactly them.
        let low = _mm512_xor_si512(unsafe { _mm512_loadu_si512(octet.as_ptr().cast()) }, first_flip);
        // SAFETY: as above.
        let high = _mm512_xor_si512(unsafe { _mm512_loadu_si512(next_octet.as_ptr().cast()) }, second_flip);
        let summed = _mm512_add_epi64(_mm512_add_epi64(len, _mm512_shuffle_epi8(low, swap)), high);
        let mut h = _mm512_add_epi64(summed, fold_avx512(low, high));
        h = _mm512_xor_si512(h, _mm512_srli_epi64::<37>(h));
        h = _mm512_mullo_epi64(h, m);
        h = _mm512_xor_si512(h, _mm512_srli_epi64::<32>(h));
        // SAFETY: `octet` is 64 bytes, and an unaligned store writes exactly them.
        unsafe { _mm512_storeu_si512(octet.as_mut_ptr().cast(), h) };
    }
    rest.iter_mut().zip(next_rest).for_each(|(h, &n)| *h = pairs.pair(*h, n));
}
