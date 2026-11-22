//! \[DESIGN\] XXH3's loops on AVX-512 Foundation (RULES.md D460). Each function here
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
    __m512i, _MM_PERM_BADC, _mm512_add_epi64, _mm512_loadu_si512, _mm512_mul_epu32, _mm512_set1_epi32, _mm512_shuffle_epi32,
    _mm512_slli_epi64, _mm512_srli_epi64, _mm512_storeu_si512, _mm512_xor_si512,
};

use turborel::hash::xxhash::{ACC_NB, Stripe, keyed};

use super::{HALF, PRIME32_1, SCRAMBLE_SHIFT, acc_bytes, acc_words, key_at};

/// The 64 bytes of `bytes`, as AVX-512 reads them.
#[target_feature(enable = "avx512f")]
fn load512(bytes: &[u8; 64]) -> __m512i {
    // SAFETY: `bytes` are 64, and an unaligned load reads exactly them.
    unsafe { _mm512_loadu_si512(bytes.as_ptr().cast()) }
}

/// Accumulates `stripes`, those whose keys `secret` holds (`keyed`), so
/// every key read lies within it.
#[target_feature(enable = "avx512f")]
pub(crate) fn xxh3_accumulate(acc: &mut [u64; ACC_NB], stripes: &[Stripe], secret: &[u8]) {
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

/// Scrambles `acc` by `key`.
#[target_feature(enable = "avx512f")]
pub(crate) fn xxh3_scramble(acc: &mut [u64; ACC_NB], key: &Stripe) {
    let a = load512(&acc_bytes(acc));
    let data_key = _mm512_xor_si512(_mm512_xor_si512(a, _mm512_srli_epi64::<SCRAMBLE_SHIFT>(a)), load512(key));
    let prime = _mm512_set1_epi32(PRIME32_1);
    let low = _mm512_mul_epu32(data_key, prime);
    let high = _mm512_mul_epu32(_mm512_srli_epi64::<HALF>(data_key), prime);
    store_avx512(acc, _mm512_add_epi64(low, _mm512_slli_epi64::<HALF>(high)));
}
