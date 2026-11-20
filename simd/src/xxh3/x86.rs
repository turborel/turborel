//! \[DESIGN\] XXH3's loops on x86-64 (`xxhash.h`'s
//! `XXH3_accumulate_512_sse2`, `_avx2`, `_avx512` and their
//! `XXH3_scrambleAcc_*`). A lane's 32-bit halves multiply into 64 bits
//! (`mul_epu32`), and its input moves to its neighbour by a shuffle of
//! 32-bit words, as the scalar loop adds it to `acc[i ^ 1]`. The
//! accumulators stay in registers across a block's stripes. Each entry
//! point checks its set, and runs the scalar loop on a processor without.
//!
//! Ported from xxHash, whose notice it keeps, as its license asks:
//!
//! ```text
//! xxHash - Extremely Fast Hash algorithm
//! Copyright (C) 2012-2023 Yann Collet
//!
//! BSD 2-Clause License (https://www.opensource.org/licenses/bsd-license.php)
//!
//! Redistribution and use in source and binary forms, with or without
//! modification, are permitted provided that the following conditions are
//! met:
//!
//!    * Redistributions of source code must retain the above copyright
//!      notice, this list of conditions and the following disclaimer.
//!    * Redistributions in binary form must reproduce the above
//!      copyright notice, this list of conditions and the following disclaimer
//!      in the documentation and/or other materials provided with the
//!      distribution.
//!
//! THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
//! "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
//! LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
//! A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
//! OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
//! SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
//! LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
//! DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
//! THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
//! (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
//! OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
//!
//! You can contact the author at:
//!   - xxHash homepage: https://www.xxhash.com
//!   - xxHash source repository: https://github.com/Cyan4973/xxHash
//! ```

use std::arch::x86_64::{
    __m128i, __m256i, __m512i, _MM_PERM_BADC, _mm_add_epi64, _mm_loadu_si128, _mm_mul_epu32, _mm_set1_epi32, _mm_shuffle_epi32,
    _mm_slli_epi64, _mm_srli_epi64, _mm_storeu_si128, _mm_xor_si128, _mm256_add_epi64, _mm256_loadu_si256, _mm256_mul_epu32,
    _mm256_set1_epi32, _mm256_shuffle_epi32, _mm256_slli_epi64, _mm256_srli_epi64, _mm256_storeu_si256, _mm256_xor_si256, _mm512_add_epi64,
    _mm512_loadu_si512, _mm512_mul_epu32, _mm512_set1_epi32, _mm512_shuffle_epi32, _mm512_slli_epi64, _mm512_srli_epi64,
    _mm512_storeu_si512, _mm512_xor_si512,
};

use turborel::hash::xxhash::{ACC_NB, SECRET_CONSUME_RATE, Scalar, Stripe, Stripes, keyed};

/// `_MM_SHUFFLE(1, 0, 3, 2)`: each lane's two 32-bit words swapped with
/// its neighbour's, the swap of `acc[i ^ 1]`.
const SWAP_LANES: i32 = 0b0100_1110;
/// `XXH_PRIME32_1`, the scramble's multiplier, as a 32-bit word.
const PRIME32_1: i32 = 0x9E37_79B1_u32.cast_signed();
/// The scramble's shift.
const SCRAMBLE_SHIFT: u32 = 47;
/// Bits in half a lane.
const HALF: u32 = 32;
/// Bytes of an SSE2 and an AVX2 register.
const XMM: usize = 16;
const YMM: usize = 32;

/// Accumulates `stripes`, each by its key in `secret`, on SSE2.
pub(super) fn accumulate_sse2(acc: &mut [u64; ACC_NB], stripes: &[Stripe], secret: &[u8]) {
    let stripes = keyed(stripes, secret);
    if std::arch::is_x86_feature_detected!("sse2") {
        // SAFETY: the processor has SSE2, checked above.
        unsafe { accumulate_with_sse2(acc, stripes, secret) }
    } else {
        Scalar.accumulate(acc, stripes, secret);
    }
}

/// Accumulates `stripes`, each by its key in `secret`, on AVX2.
pub(super) fn accumulate_avx2(acc: &mut [u64; ACC_NB], stripes: &[Stripe], secret: &[u8]) {
    let stripes = keyed(stripes, secret);
    if std::arch::is_x86_feature_detected!("avx2") {
        // SAFETY: the processor has AVX2, checked above.
        unsafe { accumulate_with_avx2(acc, stripes, secret) }
    } else {
        Scalar.accumulate(acc, stripes, secret);
    }
}

/// Accumulates `stripes`, each by its key in `secret`, on AVX-512.
pub(super) fn accumulate_avx512(acc: &mut [u64; ACC_NB], stripes: &[Stripe], secret: &[u8]) {
    let stripes = keyed(stripes, secret);
    if std::arch::is_x86_feature_detected!("avx512f") {
        // SAFETY: the processor has AVX-512 Foundation, checked above.
        unsafe { accumulate_with_avx512(acc, stripes, secret) }
    } else {
        Scalar.accumulate(acc, stripes, secret);
    }
}

/// Scrambles `acc` by `key`, on SSE2.
pub(super) fn scramble_sse2(acc: &mut [u64; ACC_NB], key: &Stripe) {
    if std::arch::is_x86_feature_detected!("sse2") {
        // SAFETY: the processor has SSE2, checked above.
        unsafe { scramble_with_sse2(acc, key) }
    } else {
        Scalar.scramble(acc, key);
    }
}

/// Scrambles `acc` by `key`, on AVX2.
pub(super) fn scramble_avx2(acc: &mut [u64; ACC_NB], key: &Stripe) {
    if std::arch::is_x86_feature_detected!("avx2") {
        // SAFETY: the processor has AVX2, checked above.
        unsafe { scramble_with_avx2(acc, key) }
    } else {
        Scalar.scramble(acc, key);
    }
}

/// Scrambles `acc` by `key`, on AVX-512.
pub(super) fn scramble_avx512(acc: &mut [u64; ACC_NB], key: &Stripe) {
    if std::arch::is_x86_feature_detected!("avx512f") {
        // SAFETY: the processor has AVX-512 Foundation, checked above.
        unsafe { scramble_with_avx512(acc, key) }
    } else {
        Scalar.scramble(acc, key);
    }
}

/// The 16 bytes of `bytes` at `at`, as SSE2 reads them.
#[target_feature(enable = "sse2")]
fn load128(bytes: &[u8; 64], at: usize) -> __m128i {
    let part = bytes.get(at..).and_then(<[u8]>::first_chunk::<XMM>).unwrap_or(&[0; XMM]);
    // SAFETY: `part` is 16 bytes, and an unaligned load reads exactly them.
    unsafe { _mm_loadu_si128(part.as_ptr().cast()) }
}

/// The 32 bytes of `bytes` at `at`, as AVX2 reads them.
#[target_feature(enable = "avx2")]
fn load256(bytes: &[u8; 64], at: usize) -> __m256i {
    let part = bytes.get(at..).and_then(<[u8]>::first_chunk::<YMM>).unwrap_or(&[0; YMM]);
    // SAFETY: `part` is 32 bytes, and an unaligned load reads exactly them.
    unsafe { _mm256_loadu_si256(part.as_ptr().cast()) }
}

/// The 64 bytes of `bytes`, as AVX-512 reads them.
#[target_feature(enable = "avx512f")]
fn load512(bytes: &[u8; 64]) -> __m512i {
    // SAFETY: `bytes` are 64, and an unaligned load reads exactly them.
    unsafe { _mm512_loadu_si512(bytes.as_ptr().cast()) }
}

/// Where the bytes at `at` of the key of the `n`th stripe start in
/// `secret`: `n` times `SECRET_CONSUME_RATE`, plus `at`.
fn key_at(secret: &[u8], n: usize, at: usize) -> *const u8 {
    secret.as_ptr().wrapping_add(n.wrapping_mul(SECRET_CONSUME_RATE).wrapping_add(at))
}

/// The accumulators as bytes, to load and store them as vectors.
fn acc_bytes(acc: &[u64; ACC_NB]) -> [u8; 64] {
    let mut bytes = [0; 64];
    bytes.as_chunks_mut::<8>().0.iter_mut().zip(acc).for_each(|(b, a)| *b = a.to_ne_bytes());
    bytes
}

/// The accumulators of their bytes, as `acc_bytes` wrote them.
fn acc_words(bytes: &[u8; 64], acc: &mut [u64; ACC_NB]) {
    acc.iter_mut().zip(bytes.as_chunks::<8>().0).for_each(|(a, b)| *a = u64::from_ne_bytes(*b));
}

/// One SSE2 register's lanes of a stripe: `XXH3_accumulate_512_sse2`.
#[target_feature(enable = "sse2")]
fn round_sse2(acc: __m128i, data: __m128i, key: __m128i) -> __m128i {
    let data_key = _mm_xor_si128(data, key);
    let product = _mm_mul_epu32(data_key, _mm_srli_epi64::<HALF_I32>(data_key));
    _mm_add_epi64(product, _mm_add_epi64(acc, _mm_shuffle_epi32::<SWAP_LANES>(data)))
}

/// One SSE2 register's lanes scrambled: `XXH3_scrambleAcc_sse2`.
#[target_feature(enable = "sse2")]
fn scramble_lane_sse2(acc: __m128i, key: __m128i) -> __m128i {
    let data_key = _mm_xor_si128(_mm_xor_si128(acc, _mm_srli_epi64::<SHIFT_I32>(acc)), key);
    let prime = _mm_set1_epi32(PRIME32_1);
    let low = _mm_mul_epu32(data_key, prime);
    let high = _mm_mul_epu32(_mm_srli_epi64::<HALF_I32>(data_key), prime);
    _mm_add_epi64(low, _mm_slli_epi64::<HALF_I32>(high))
}

/// The half-lane shift, as the intrinsics take it.
const HALF_I32: i32 = HALF.cast_signed();
/// The scramble's shift, as the intrinsics take it.
const SHIFT_I32: i32 = SCRAMBLE_SHIFT.cast_signed();

/// Accumulates `stripes`, those whose keys `secret` holds (`keyed`), so
/// every key read lies within it.
#[target_feature(enable = "sse2")]
fn accumulate_with_sse2(acc: &mut [u64; ACC_NB], stripes: &[Stripe], secret: &[u8]) {
    let stripes = keyed(stripes, secret);
    let bytes = acc_bytes(acc);
    let mut regs = [load128(&bytes, 0), load128(&bytes, XMM), load128(&bytes, 2 * XMM), load128(&bytes, 3 * XMM)];
    for (n, stripe) in stripes.iter().enumerate() {
        for (part, reg) in regs.iter_mut().enumerate() {
            let at = part.wrapping_mul(XMM);
            // SAFETY: the key of stripe `n`, 64 bytes at `n` times 8, lies in
            // `secret`, as `stripes` are those it keys (`keyed`); these are
            // 16 of them, read unaligned.
            let key = unsafe { _mm_loadu_si128(key_at(secret, n, at).cast()) };
            *reg = round_sse2(*reg, load128(stripe, at), key);
        }
    }
    store_sse2(acc, regs);
}

/// Stores the four SSE2 registers into the accumulators.
#[target_feature(enable = "sse2")]
fn store_sse2(acc: &mut [u64; ACC_NB], regs: [__m128i; 4]) {
    let mut bytes = [0_u8; 64];
    for (slot, reg) in bytes.as_chunks_mut::<XMM>().0.iter_mut().zip(regs) {
        // SAFETY: `slot` is 16 bytes, and an unaligned store writes exactly them.
        unsafe { _mm_storeu_si128(slot.as_mut_ptr().cast(), reg) }
    }
    acc_words(&bytes, acc);
}

#[target_feature(enable = "sse2")]
fn scramble_with_sse2(acc: &mut [u64; ACC_NB], key: &Stripe) {
    let bytes = acc_bytes(acc);
    let regs: [__m128i; 4] = std::array::from_fn(|n| {
        let at = n.wrapping_mul(XMM);
        scramble_lane_sse2(load128(&bytes, at), load128(key, at))
    });
    store_sse2(acc, regs);
}

#[target_feature(enable = "avx2")]
fn round_avx2(acc: __m256i, data: __m256i, key: __m256i) -> __m256i {
    let data_key = _mm256_xor_si256(data, key);
    let product = _mm256_mul_epu32(data_key, _mm256_srli_epi64::<HALF_I32>(data_key));
    _mm256_add_epi64(product, _mm256_add_epi64(acc, _mm256_shuffle_epi32::<SWAP_LANES>(data)))
}

/// Accumulates `stripes`, those whose keys `secret` holds (`keyed`), so
/// every key read lies within it.
#[target_feature(enable = "avx2")]
fn accumulate_with_avx2(acc: &mut [u64; ACC_NB], stripes: &[Stripe], secret: &[u8]) {
    let stripes = keyed(stripes, secret);
    let bytes = acc_bytes(acc);
    let (mut low, mut high) = (load256(&bytes, 0), load256(&bytes, YMM));
    for (n, stripe) in stripes.iter().enumerate() {
        // SAFETY: the key of stripe `n` lies in `secret` (`keyed`); these
        // are its first 32 bytes, read unaligned.
        let key_low = unsafe { _mm256_loadu_si256(key_at(secret, n, 0).cast()) };
        // SAFETY: as above, its last 32 bytes.
        let key_high = unsafe { _mm256_loadu_si256(key_at(secret, n, YMM).cast()) };
        low = round_avx2(low, load256(stripe, 0), key_low);
        high = round_avx2(high, load256(stripe, YMM), key_high);
    }
    store_avx2(acc, [low, high]);
}

/// Stores the two AVX2 registers into the accumulators.
#[target_feature(enable = "avx2")]
fn store_avx2(acc: &mut [u64; ACC_NB], regs: [__m256i; 2]) {
    let mut bytes = [0_u8; 64];
    for (slot, reg) in bytes.as_chunks_mut::<YMM>().0.iter_mut().zip(regs) {
        // SAFETY: `slot` is 32 bytes, and an unaligned store writes exactly them.
        unsafe { _mm256_storeu_si256(slot.as_mut_ptr().cast(), reg) }
    }
    acc_words(&bytes, acc);
}

#[target_feature(enable = "avx2")]
fn scramble_with_avx2(acc: &mut [u64; ACC_NB], key: &Stripe) {
    let bytes = acc_bytes(acc);
    let prime = _mm256_set1_epi32(PRIME32_1);
    let regs: [__m256i; 2] = std::array::from_fn(|n| {
        let at = n.wrapping_mul(YMM);
        let a = load256(&bytes, at);
        let data_key = _mm256_xor_si256(_mm256_xor_si256(a, _mm256_srli_epi64::<SHIFT_I32>(a)), load256(key, at));
        let low = _mm256_mul_epu32(data_key, prime);
        let high = _mm256_mul_epu32(_mm256_srli_epi64::<HALF_I32>(data_key), prime);
        _mm256_add_epi64(low, _mm256_slli_epi64::<HALF_I32>(high))
    });
    store_avx2(acc, regs);
}

/// Accumulates `stripes`, those whose keys `secret` holds (`keyed`), so
/// every key read lies within it.
#[target_feature(enable = "avx512f")]
fn accumulate_with_avx512(acc: &mut [u64; ACC_NB], stripes: &[Stripe], secret: &[u8]) {
    let stripes = keyed(stripes, secret);
    let mut reg = load512(&acc_bytes(acc));
    for (n, stripe) in stripes.iter().enumerate() {
        let data = load512(stripe);
        // SAFETY: the key of stripe `n`, 64 bytes, lies in `secret`
        // (`keyed`), read unaligned.
        let key = unsafe { _mm512_loadu_si512(key_at(secret, n, 0).cast()) };
        let data_key = _mm512_xor_si512(data, key);
        let product = _mm512_mul_epu32(data_key, _mm512_srli_epi64::<HALF>(data_key));
        reg = _mm512_add_epi64(product, _mm512_add_epi64(reg, _mm512_shuffle_epi32::<_MM_PERM_BADC>(data)));
    }
    store_avx512(acc, reg);
}

/// Stores the AVX-512 register into the accumulators.
#[target_feature(enable = "avx512f")]
fn store_avx512(acc: &mut [u64; ACC_NB], reg: __m512i) {
    let mut bytes = [0_u8; 64];
    // SAFETY: `bytes` are 64, and an unaligned store writes exactly them.
    unsafe { _mm512_storeu_si512(bytes.as_mut_ptr().cast(), reg) }
    acc_words(&bytes, acc);
}

#[target_feature(enable = "avx512f")]
fn scramble_with_avx512(acc: &mut [u64; ACC_NB], key: &Stripe) {
    let a = load512(&acc_bytes(acc));
    let data_key = _mm512_xor_si512(_mm512_xor_si512(a, _mm512_srli_epi64::<SCRAMBLE_SHIFT>(a)), load512(key));
    let prime = _mm512_set1_epi32(PRIME32_1);
    let low = _mm512_mul_epu32(data_key, prime);
    let high = _mm512_mul_epu32(_mm512_srli_epi64::<HALF>(data_key), prime);
    store_avx512(acc, _mm512_add_epi64(low, _mm512_slli_epi64::<HALF>(high)));
}
