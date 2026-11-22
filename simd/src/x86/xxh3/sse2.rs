//! \[DESIGN\] XXH3's loops on SSE2 (RULES.md D460). Each function here
//! is called only by a `Simd` of this level, which exists only where the
//! processor has it (`Simd::detect`, `Simd::with_level`).
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
    __m128i, _mm_add_epi64, _mm_loadu_si128, _mm_mul_epu32, _mm_set1_epi32, _mm_shuffle_epi32, _mm_slli_epi64, _mm_srli_epi64,
    _mm_storeu_si128, _mm_xor_si128,
};

use turborel::hash::xxhash::{ACC_NB, Stripe, keyed};

use super::{HALF_I32, PRIME32_1, SHIFT_I32, SWAP_LANES, acc_bytes, acc_words, key_at};

/// Bytes of an SSE2 register.
const XMM: usize = 16;

/// The 16 bytes of `bytes` at `at`, as SSE2 reads them.
#[target_feature(enable = "sse2")]
fn load128(bytes: &[u8; 64], at: usize) -> __m128i {
    let part = bytes.get(at..).and_then(<[u8]>::first_chunk::<XMM>).unwrap_or(&[0; XMM]);
    // SAFETY: `part` is 16 bytes, and an unaligned load reads exactly them.
    unsafe { _mm_loadu_si128(part.as_ptr().cast()) }
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

/// Accumulates `stripes`, those whose keys `secret` holds (`keyed`), so
/// every key read lies within it.
#[target_feature(enable = "sse2")]
pub(crate) fn xxh3_accumulate(acc: &mut [u64; ACC_NB], stripes: &[Stripe], secret: &[u8]) {
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

/// Scrambles `acc` by `key`.
#[target_feature(enable = "sse2")]
pub(crate) fn xxh3_scramble(acc: &mut [u64; ACC_NB], key: &Stripe) {
    let bytes = acc_bytes(acc);
    let regs: [__m128i; 4] = std::array::from_fn(|n| {
        let at = n.wrapping_mul(XMM);
        scramble_lane_sse2(load128(&bytes, at), load128(key, at))
    });
    store_sse2(acc, regs);
}
