//! \[DESIGN\] XXH3 of 17 to 240 bytes (`xxhash.h`'s
//! `XXH3_len_17to128_64b` to `XXH3_len_129to240_128b`; the
//! specification's "medium inputs"): chunks of 16 bytes, each mixed with
//! 16 of the secret by one multiplication. The input and the secret are
//! read as arrays of chunks, so the compiler sees no bound to check.
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

use super::short::Head;
use super::{PRIME64_1, PRIME64_2, PRIME64_4, SECRET_SIZE_MIN, avalanche, joined, len64, mul128_fold64};

/// Bytes of a chunk, `XXH3_mix16B`'s.
const MIX: usize = 16;
/// Bytes of a pair of chunks, `XXH128_mix32B`'s.
const PAIR: usize = 2 * MIX;
/// A chunk.
type Chunk = [u8; MIX];
/// `XXH3_MIDSIZE_STARTOFFSET`: where the secret of 129 to 240 bytes'
/// chunks past the eighth start.
const MIDSIZE_STARTOFFSET: usize = 3;
/// `XXH3_MIDSIZE_LASTOFFSET`: how far before `SECRET_SIZE_MIN` the key of
/// the last 16 bytes of 129 to 240 starts.
const MIDSIZE_LASTOFFSET: usize = 17;
/// The chunks 129 to 240 bytes mix before their first avalanche.
const MID_FIRST: usize = 8;
/// The pairs of chunks XXH3-128 of 129 to 240 bytes mixes before it.
const MID_FIRST_PAIRS: usize = 4;
/// No chunk: the fallback of a read the lengths rule out.
const NONE: Chunk = [0; MIX];

/// The two little-endian words of a chunk.
#[inline]
fn words(chunk: &Chunk) -> (u64, u64) {
    let low = chunk.first_chunk::<8>().map_or(0, |w| u64::from_le_bytes(*w));
    let high = chunk.last_chunk::<8>().map_or(0, |w| u64::from_le_bytes(*w));
    (low, high)
}

/// `XXH3_mix16B`: `chunk` mixed with `key` and `seed`.
#[inline]
fn mix(chunk: &Chunk, key: &Chunk, seed: u64) -> u64 {
    let ((low, high), (key_low, key_high)) = (words(chunk), words(key));
    mul128_fold64(low ^ key_low.wrapping_add(seed), high ^ key_high.wrapping_sub(seed))
}

/// The 16 bytes of `bytes` at `at`.
#[inline]
fn chunk_at(bytes: &[u8], at: usize) -> &Chunk {
    bytes.get(at..).and_then(<[u8]>::first_chunk::<MIX>).unwrap_or(&NONE)
}

/// The chunks of `secret` from `at` on.
#[inline]
fn keys_from(secret: &Head, at: usize) -> &[Chunk] {
    secret.get(at..).unwrap_or_default().as_chunks::<MIX>().0
}

/// The `n`th 16 bytes of `input` from its start and from its end, as 17 to
/// 128 bytes pair them.
#[inline]
fn ends(input: &[u8], n: usize) -> (&Chunk, &Chunk) {
    let from = n.wrapping_mul(MIX);
    let back = input.get(..input.len().wrapping_sub(from)).and_then(<[u8]>::last_chunk::<MIX>).unwrap_or(&NONE);
    (chunk_at(input, from), back)
}

/// The pairs 17 to 128 bytes mix, less one: one more for each 32 bytes
/// past the first 32.
#[inline]
fn rounds_17to128(len: usize) -> usize {
    (len.wrapping_sub(1) >> 5_u32).min(3)
}

/// `XXH3_len_17to128_64b`: pairs of 16 bytes from either end.
#[inline]
pub(super) fn hash64_17to128(input: &[u8], secret: &Head, seed: u64) -> u64 {
    let len = input.len();
    let mut acc = len64(len).wrapping_mul(PRIME64_1);
    let keys = keys_from(secret, 0);
    for n in (0..=rounds_17to128(len)).rev() {
        let (front, back) = ends(input, n);
        let key = n.wrapping_mul(2);
        acc = acc.wrapping_add(mix(front, keys.get(key).unwrap_or(&NONE), seed));
        acc = acc.wrapping_add(mix(back, keys.get(key.wrapping_add(1)).unwrap_or(&NONE), seed));
    }
    avalanche(acc)
}

/// `XXH3_len_129to240_64b`: the first eight chunks, an avalanche, the
/// rest, and the last 16 bytes.
#[inline(never)]
pub(super) fn hash64_129to240(input: &[u8], secret: &Head, seed: u64) -> u64 {
    let len = input.len();
    let (chunks, _) = input.as_chunks::<MIX>();
    let (first, rest) = chunks.split_at_checked(MID_FIRST).unwrap_or_default();
    let mut acc = len64(len).wrapping_mul(PRIME64_1);
    for (chunk, key) in first.iter().zip(keys_from(secret, 0)) {
        acc = acc.wrapping_add(mix(chunk, key, seed));
    }
    let last_key = chunk_at(secret, SECRET_SIZE_MIN.wrapping_sub(MIDSIZE_LASTOFFSET));
    let mut acc_end = mix(input.last_chunk::<MIX>().unwrap_or(&NONE), last_key, seed);
    for (chunk, key) in rest.iter().zip(keys_from(secret, MIDSIZE_STARTOFFSET)) {
        acc_end = acc_end.wrapping_add(mix(chunk, key, seed));
    }
    avalanche(avalanche(acc).wrapping_add(acc_end))
}

/// `XXH128_mix32B`: `first` and `second` into the accumulators `acc`,
/// by the 32 bytes `key`.
#[inline]
fn mix_pair(acc: (u64, u64), (first, second): (&Chunk, &Chunk), key: &[u8; PAIR], seed: u64) -> (u64, u64) {
    let (key_first, key_second) = (key.first_chunk::<MIX>().unwrap_or(&NONE), key.last_chunk::<MIX>().unwrap_or(&NONE));
    let sum = |chunk: &Chunk| {
        let (low, high) = words(chunk);
        low.wrapping_add(high)
    };
    let low = acc.0.wrapping_add(mix(first, key_first, seed)) ^ sum(second);
    let high = acc.1.wrapping_add(mix(second, key_second, seed)) ^ sum(first);
    (low, high)
}

/// The halves of 32 bytes.
#[inline]
fn halves(pair: &[u8; PAIR]) -> (&Chunk, &Chunk) {
    (pair.first_chunk::<MIX>().unwrap_or(&NONE), pair.last_chunk::<MIX>().unwrap_or(&NONE))
}

/// The finish of XXH3-128 of 17 to 240 bytes from its accumulators.
#[inline]
fn finish128((low, high): (u64, u64), len: usize, seed: u64) -> u128 {
    let out_low = low.wrapping_add(high);
    let out_high = low
        .wrapping_mul(PRIME64_1)
        .wrapping_add(high.wrapping_mul(PRIME64_4))
        .wrapping_add(len64(len).wrapping_sub(seed).wrapping_mul(PRIME64_2));
    joined(avalanche(out_low), 0_u64.wrapping_sub(avalanche(out_high)))
}

/// `XXH3_len_17to128_128b`.
#[inline]
pub(super) fn hash128_17to128(input: &[u8], secret: &Head, seed: u64) -> u128 {
    let len = input.len();
    let mut acc = (len64(len).wrapping_mul(PRIME64_1), 0_u64);
    let (keys, _) = secret.as_chunks::<PAIR>();
    for n in (0..=rounds_17to128(len)).rev() {
        acc = mix_pair(acc, ends(input, n), keys.get(n).unwrap_or(&[0; PAIR]), seed);
    }
    finish128(acc, len, seed)
}

/// `XXH3_len_129to240_128b`: the first four pairs, an avalanche of each
/// accumulator, the rest, and the last 32 bytes, their halves swapped.
#[inline(never)]
pub(super) fn hash128_129to240(input: &[u8], secret: &Head, seed: u64) -> u128 {
    let len = input.len();
    let (pairs, _) = input.as_chunks::<PAIR>();
    let (first, rest) = pairs.split_at_checked(MID_FIRST_PAIRS).unwrap_or_default();
    let mut acc = (len64(len).wrapping_mul(PRIME64_1), 0_u64);
    for (pair, key) in first.iter().zip(secret.as_chunks::<PAIR>().0) {
        acc = mix_pair(acc, halves(pair), key, seed);
    }
    acc = (avalanche(acc.0), avalanche(acc.1));
    let rest_keys = secret.get(MIDSIZE_STARTOFFSET..).unwrap_or_default().as_chunks::<PAIR>().0;
    for (pair, key) in rest.iter().zip(rest_keys) {
        acc = mix_pair(acc, halves(pair), key, seed);
    }
    let last_key = SECRET_SIZE_MIN.wrapping_sub(MIDSIZE_LASTOFFSET).wrapping_sub(MIX);
    let key = secret.get(last_key..).and_then(<[u8]>::first_chunk::<PAIR>).unwrap_or(&[0; PAIR]);
    let (before_last, last) = halves(input.last_chunk::<PAIR>().unwrap_or(&[0; PAIR]));
    acc = mix_pair(acc, (last, before_last), key, 0_u64.wrapping_sub(seed));
    finish128(acc, len, seed)
}
