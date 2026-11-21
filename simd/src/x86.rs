//! \[DESIGN\] Kernels on x86-64: SSE2, which every such processor has,
//! AVX2 and AVX-512 Foundation, a module each of `#[target_feature]`
//! functions, and what they share. XXH3's loops (`xxhash.h`'s
//! `XXH3_accumulate_512_sse2`, `_avx2`, `_avx512` and their
//! `XXH3_scrambleAcc_*`): a lane's 32-bit halves multiply into 64 bits
//! (`mul_epu32`), and its input moves to its neighbour by a shuffle of
//! 32-bit words, as the scalar loop adds it to `acc[i ^ 1]`; the
//! accumulators stay in registers across a block's stripes.
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

use turborel::hash::xxhash::{ACC_NB, SECRET_CONSUME_RATE};

pub(crate) mod avx2;
pub(crate) mod avx512;
pub(crate) mod sse2;

/// `_MM_SHUFFLE(1, 0, 3, 2)`: each lane's two 32-bit words swapped with
/// its neighbour's, the swap of `acc[i ^ 1]`.
pub(crate) const SWAP_LANES: i32 = 0b0100_1110;
/// `XXH_PRIME32_1`, the scramble's multiplier, as a 32-bit word.
pub(crate) const PRIME32_1: i32 = 0x9E37_79B1_u32.cast_signed();
/// The scramble's shift.
pub(crate) const SCRAMBLE_SHIFT: u32 = 47;
/// Bits in half a lane.
pub(crate) const HALF: u32 = 32;
/// The half-lane shift, as the intrinsics take it.
pub(crate) const HALF_I32: i32 = HALF.cast_signed();
/// The scramble's shift, as the intrinsics take it.
pub(crate) const SHIFT_I32: i32 = SCRAMBLE_SHIFT.cast_signed();

/// Where the bytes at `at` of the key of the `n`th stripe start in
/// `secret`: `n` times `SECRET_CONSUME_RATE`, plus `at`.
pub(crate) fn key_at(secret: &[u8], n: usize, at: usize) -> *const u8 {
    secret.as_ptr().wrapping_add(n.wrapping_mul(SECRET_CONSUME_RATE).wrapping_add(at))
}

/// The accumulators as bytes, to load and store them as vectors.
pub(crate) fn acc_bytes(acc: &[u64; ACC_NB]) -> [u8; 64] {
    let mut bytes = [0; 64];
    bytes.as_chunks_mut::<8>().0.iter_mut().zip(acc).for_each(|(b, a)| *b = a.to_ne_bytes());
    bytes
}

/// The accumulators of their bytes, as `acc_bytes` wrote them.
pub(crate) fn acc_words(bytes: &[u8; 64], acc: &mut [u64; ACC_NB]) {
    acc.iter_mut().zip(bytes.as_chunks::<8>().0).for_each(|(a, b)| *a = u64::from_ne_bytes(*b));
}
