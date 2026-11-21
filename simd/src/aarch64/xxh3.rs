//! \[DESIGN\] XXH3's loops on AArch64 NEON (`xxhash.h`'s
//! `XXH3_accumulate_512_neon` and `XXH3_scrambleAcc_neon`): each register
//! holds two lanes, its input's halves swapped by `vextq_u64`, and a
//! lane's 32-bit halves multiplied and added at once by `vmlal_u32`. Each
//! function here is called only by a `Simd` of NEON, which exists only
//! where the processor has it.
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

use std::arch::aarch64::{
    uint64x2_t, vaddq_u64, vdup_n_u32, veorq_u64, vextq_u64, vld1q_u64, vmlal_u32, vmovn_u64, vmull_u32, vshlq_n_u64, vshrn_n_u64,
    vshrq_n_u64, vst1q_u64,
};

use turborel::hash::xxhash::{ACC_NB, SECRET_CONSUME_RATE, STRIPE_LEN, Stripe};

/// `XXH_PRIME32_1`, the scramble's multiplier.
const PRIME32_1: u32 = 0x9E37_79B1;
/// Lanes of a register.
const LANES: usize = 2;

/// The two little-endian words of each 16 bytes of `bytes`.
fn words(bytes: &[u8; 64]) -> [[u64; LANES]; 4] {
    let mut out = [[0; LANES]; 4];
    for (pair, chunk) in out.iter_mut().zip(bytes.as_chunks::<16>().0) {
        for (word, b) in pair.iter_mut().zip(chunk.as_chunks::<8>().0) {
            *word = u64::from_le_bytes(*b);
        }
    }
    out
}

/// The register of two words.
#[target_feature(enable = "neon")]
fn load(pair: &[u64; LANES]) -> uint64x2_t {
    // SAFETY: `pair` is two words, and the load reads exactly them.
    unsafe { vld1q_u64(pair.as_ptr()) }
}

/// The accumulators' registers.
#[target_feature(enable = "neon")]
fn loaded(acc: &[u64; ACC_NB]) -> [uint64x2_t; 4] {
    let (pairs, _) = acc.as_chunks::<LANES>();
    std::array::from_fn(|n| pairs.get(n).map_or_else(|| load(&[0; LANES]), |pair| load(pair)))
}

/// Stores the registers into the accumulators.
#[target_feature(enable = "neon")]
fn store(acc: &mut [u64; ACC_NB], regs: [uint64x2_t; 4]) {
    for (pair, reg) in acc.as_chunks_mut::<LANES>().0.iter_mut().zip(regs) {
        // SAFETY: `pair` is two words, and the store writes exactly them.
        unsafe { vst1q_u64(pair.as_mut_ptr(), reg) }
    }
}

/// Accumulates `stripes`, each by its key in `secret`.
#[target_feature(enable = "neon")]
pub(crate) fn xxh3_accumulate(acc: &mut [u64; ACC_NB], stripes: &[Stripe], secret: &[u8]) {
    let mut regs = loaded(acc);
    let stripe_keys = secret.windows(STRIPE_LEN).step_by(SECRET_CONSUME_RATE).filter_map(<[u8]>::first_chunk::<STRIPE_LEN>);
    for (stripe, stripe_key) in stripes.iter().zip(stripe_keys) {
        let (inputs, lane_keys) = (words(stripe), words(stripe_key));
        for ((reg, input), lane_key) in regs.iter_mut().zip(&inputs).zip(&lane_keys) {
            let lanes = load(input);
            let data_key = veorq_u64(lanes, load(lane_key));
            let sum = vaddq_u64(*reg, vextq_u64::<1>(lanes, lanes));
            *reg = vmlal_u32(sum, vmovn_u64(data_key), vshrn_n_u64::<32>(data_key));
        }
    }
    store(acc, regs);
}

/// Scrambles `acc` by `key`.
#[target_feature(enable = "neon")]
pub(crate) fn xxh3_scramble(acc: &mut [u64; ACC_NB], key: &Stripe) {
    let lane_keys = words(key);
    let prime = vdup_n_u32(PRIME32_1);
    let mut regs = loaded(acc);
    for (reg, lane_key) in regs.iter_mut().zip(&lane_keys) {
        let data_key = veorq_u64(veorq_u64(*reg, vshrq_n_u64::<47>(*reg)), load(lane_key));
        let high = vshlq_n_u64::<32>(vmull_u32(vshrn_n_u64::<32>(data_key), prime));
        *reg = vmlal_u32(high, vmovn_u64(data_key), prime);
    }
    store(acc, regs);
}
