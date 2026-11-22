//! \[DESIGN\] Decimals made approximate numbers on AVX2 and AVX-512
//! (RULES.md D353, D570): each n, of magnitude below 2^51, made an `f64`
//! by adding its bits to those of 2^52 + 2^51 and taking that number away,
//! then multiplied by its chunk's factors or divided by its power, four or
//! eight a step: the portable body's operations in its order, no fused
//! multiply, so every value is bit for bit the portable one's. A function
//! of a set is called only by an `Isa` of it, made only where the
//! processor has it (`x86::isa`).

use std::arch::x86_64::{
    __m256d, __m512d, _mm256_add_epi64, _mm256_castpd_si256, _mm256_castsi256_pd, _mm256_div_pd, _mm256_loadu_si256, _mm256_mul_pd,
    _mm256_set1_epi64x, _mm256_set1_pd, _mm256_storeu_si256, _mm256_sub_pd, _mm512_add_epi64, _mm512_castpd_si512, _mm512_castsi512_pd,
    _mm512_div_pd, _mm512_loadu_si512, _mm512_mul_pd, _mm512_set1_epi64, _mm512_set1_pd, _mm512_storeu_si512, _mm512_sub_pd,
};

use turborel::kernels::{AlpScale, portable};

/// The bits of 2^52 + 2^51.
const MAGIC_BITS: i64 = 0x4338_0000_0000_0000;
/// 2^52 + 2^51.
const MAGIC: f64 = 6_755_399_441_055_744.0;

/// `Kernels::alp_decode` on AVX2.
pub(super) fn decode_avx2(words: &mut [i64], scale: AlpScale) {
    // SAFETY: an `Isa` of AVX2 is made only where the processor has it
    // (`x86::isa`), and only such a one calls this.
    unsafe { decoded_avx2(words, scale) }
}

/// Of `n`, made an `f64` as the magic number makes it, its value by `scale`.
#[target_feature(enable = "avx2")]
fn scaled_avx2(n: __m256d, scale: AlpScale) -> __m256d {
    match scale {
        AlpScale::Factored(power, fraction) => _mm256_mul_pd(_mm256_mul_pd(n, _mm256_set1_pd(power)), _mm256_set1_pd(fraction)),
        AlpScale::Divided(power) => _mm256_div_pd(n, _mm256_set1_pd(power)),
    }
}

/// `decode_avx2`'s loop.
#[target_feature(enable = "avx2")]
fn decoded_avx2(words: &mut [i64], scale: AlpScale) {
    let (quads, tail) = words.as_chunks_mut::<4>();
    let (bits, magic) = (_mm256_set1_epi64x(MAGIC_BITS), _mm256_set1_pd(MAGIC));
    for quad in quads.iter_mut() {
        // SAFETY: `quad` is 32 bytes, and an unaligned load reads exactly them.
        let v = unsafe { _mm256_loadu_si256(quad.as_ptr().cast()) };
        let n = _mm256_sub_pd(_mm256_castsi256_pd(_mm256_add_epi64(v, bits)), magic);
        let value = _mm256_castpd_si256(scaled_avx2(n, scale));
        // SAFETY: `quad` is 32 bytes, and an unaligned store writes exactly them.
        unsafe { _mm256_storeu_si256(quad.as_mut_ptr().cast(), value) };
    }
    portable::alp::alp_decode(tail, scale);
}

/// `Kernels::alp_decode` on AVX-512.
pub(super) fn decode_avx512(words: &mut [i64], scale: AlpScale) {
    // SAFETY: an `Isa` of AVX-512 is made only where the processor has it
    // (`x86::isa`), and only such a one calls this.
    unsafe { decoded_avx512(words, scale) }
}

/// Of `n`, made an `f64` as the magic number makes it, its value by `scale`.
#[target_feature(enable = "avx512f")]
fn scaled_avx512(n: __m512d, scale: AlpScale) -> __m512d {
    match scale {
        AlpScale::Factored(power, fraction) => _mm512_mul_pd(_mm512_mul_pd(n, _mm512_set1_pd(power)), _mm512_set1_pd(fraction)),
        AlpScale::Divided(power) => _mm512_div_pd(n, _mm512_set1_pd(power)),
    }
}

/// `decode_avx512`'s loop.
#[target_feature(enable = "avx512f")]
fn decoded_avx512(words: &mut [i64], scale: AlpScale) {
    let (octets, tail) = words.as_chunks_mut::<8>();
    let (bits, magic) = (_mm512_set1_epi64(MAGIC_BITS), _mm512_set1_pd(MAGIC));
    for octet in octets.iter_mut() {
        // SAFETY: `octet` is 64 bytes, and an unaligned load reads exactly them.
        let v = unsafe { _mm512_loadu_si512(octet.as_ptr().cast()) };
        let n = _mm512_sub_pd(_mm512_castsi512_pd(_mm512_add_epi64(v, bits)), magic);
        let value = _mm512_castpd_si512(scaled_avx512(n, scale));
        // SAFETY: `octet` is 64 bytes, and an unaligned store writes exactly them.
        unsafe { _mm512_storeu_si512(octet.as_mut_ptr().cast(), value) };
    }
    portable::alp::alp_decode(tail, scale);
}
