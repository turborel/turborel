//! \[DESIGN\] XXH3's loops on AVX2 (RULES.md D460). Each function here
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
    __m256i, _mm256_add_epi64, _mm256_loadu_si256, _mm256_mul_epu32, _mm256_set1_epi32, _mm256_shuffle_epi32, _mm256_slli_epi64,
    _mm256_srli_epi64, _mm256_storeu_si256, _mm256_xor_si256,
};

use turborel::hash::xxhash::{ACC_NB, Stripe, keyed};

use super::{HALF_I32, PRIME32_1, SHIFT_I32, SWAP_LANES, acc_bytes, acc_words, key_at};

/// Bytes of an AVX2 register.
const YMM: usize = 32;

/// The 32 bytes of `bytes` at `at`, as AVX2 reads them.
#[target_feature(enable = "avx2")]
fn load256(bytes: &[u8; 64], at: usize) -> __m256i {
    let part = bytes.get(at..).and_then(<[u8]>::first_chunk::<YMM>).unwrap_or(&[0; YMM]);
    // SAFETY: `part` is 32 bytes, and an unaligned load reads exactly them.
    unsafe { _mm256_loadu_si256(part.as_ptr().cast()) }
}

/// One AVX2 register's lanes of a stripe: `XXH3_accumulate_512_avx2`.
#[target_feature(enable = "avx2")]
fn round_avx2(acc: __m256i, data: __m256i, key: __m256i) -> __m256i {
    let data_key = _mm256_xor_si256(data, key);
    let product = _mm256_mul_epu32(data_key, _mm256_srli_epi64::<HALF_I32>(data_key));
    _mm256_add_epi64(product, _mm256_add_epi64(acc, _mm256_shuffle_epi32::<SWAP_LANES>(data)))
}

/// Accumulates `stripes`, those whose keys `secret` holds (`keyed`), so
/// every key read lies within it.
#[target_feature(enable = "avx2")]
pub(crate) fn xxh3_accumulate(acc: &mut [u64; ACC_NB], stripes: &[Stripe], secret: &[u8]) {
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

/// Scrambles `acc` by `key`.
#[target_feature(enable = "avx2")]
pub(crate) fn xxh3_scramble(acc: &mut [u64; ACC_NB], key: &Stripe) {
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
