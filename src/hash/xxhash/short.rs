//! \[DESIGN\] XXH3 of inputs of up to 240 bytes (`xxhash.h`'s
//! `XXH3_len_0to16_64b` to `XXH3_len_129to240_128b`; the specification's
//! "small inputs" and "medium inputs"): a few multiplications of the
//! input's words, read from both ends, with words of the secret's first
//! `SECRET_SIZE_MIN` bytes.
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

use super::{
    PRIME_MX2, PRIME32_2, PRIME64_1, PRIME64_2, SECRET_SIZE_MIN, avalanche, avalanche_xxh64, joined, le32, le64, len64, mul128,
    mul128_fold64, rrmxmx,
};

/// The secret's bytes short inputs read.
pub(super) type Head = [u8; SECRET_SIZE_MIN];

/// The byte at `at` of `input`, 0 past its end.
#[inline]
fn byte(input: &[u8], at: usize) -> u32 {
    input.get(at).copied().map_or(0, u32::from)
}

/// `XXH3_len_1to3`'s `combined`: the last byte, the length, the first
/// byte and the middle one, from the low byte up.
#[inline]
fn combined_1to3(input: &[u8]) -> u32 {
    let len = input.len();
    let first = byte(input, 0);
    let middle = byte(input, len >> 1_u32);
    let last = byte(input, len.wrapping_sub(1));
    (first << 16_u32) | (middle << 24_u32) | last | (u32::try_from(len).unwrap_or(0) << 8_u32)
}

/// The 4 to 8 bytes' seed: its low half's bytes swapped into its high.
#[inline]
fn seed_4to8(seed: u64) -> u64 {
    let low = u32::try_from(seed & u64::from(u32::MAX)).unwrap_or(0);
    seed ^ (u64::from(low.swap_bytes()) << 32_u32)
}

/// XXH3-64's `bitflip` of 4 to 8 bytes, of `secret` and `seed`.
#[inline]
pub(super) fn bitflip_4to8(secret: &[u8], seed: u64) -> u64 {
    (le64(secret, 8) ^ le64(secret, 16)).wrapping_sub(seed_4to8(seed))
}

/// XXH3-64's `bitflip1` and `bitflip2` of 9 to 16 bytes, of `secret` and
/// `seed`.
#[inline]
pub(super) fn bitflips_9to16(secret: &[u8], seed: u64) -> (u64, u64) {
    ((le64(secret, 24) ^ le64(secret, 32)).wrapping_add(seed), (le64(secret, 40) ^ le64(secret, 48)).wrapping_sub(seed))
}

/// `XXH3_len_9to16_64b` of an input of `len` bytes whose first and last
/// eight are `first` and `last`, by its `bitflips`.
#[inline]
pub(super) fn hash64_9to16_words((first, last): (u64, u64), (flip1, flip2): (u64, u64), len: u64) -> u64 {
    let (low, high) = (first ^ flip1, last ^ flip2);
    avalanche(len.wrapping_add(low.swap_bytes()).wrapping_add(high).wrapping_add(mul128_fold64(low, high)))
}

/// The 4 to 8 bytes' words: the first four bytes and the last four.
#[inline]
fn ends32(input: &[u8]) -> (u64, u64) {
    (u64::from(le32(input, 0)), u64::from(le32(input, input.len().wrapping_sub(4))))
}

/// The 9 to 16 bytes' words: the first eight bytes and the last eight.
#[inline]
fn ends64(input: &[u8]) -> (u64, u64) {
    (le64(input, 0), le64(input, input.len().wrapping_sub(8)))
}

/// `XXH3_len_0to16_64b`.
#[inline]
pub(super) fn hash64_0to16(input: &[u8], secret: &Head, seed: u64) -> u64 {
    let len = len64(input.len());
    match input.len() {
        0 => avalanche_xxh64(seed ^ le64(secret, 56) ^ le64(secret, 64)),
        1..=3 => {
            let bitflip = u64::from(le32(secret, 0) ^ le32(secret, 4)).wrapping_add(seed);
            avalanche_xxh64(u64::from(combined_1to3(input)) ^ bitflip)
        }
        4..=8 => {
            let (first, last) = ends32(input);
            rrmxmx(last.wrapping_add(first << 32_u32) ^ bitflip_4to8(secret, seed), len)
        }
        _ => hash64_9to16_words(ends64(input), bitflips_9to16(secret, seed), len),
    }
}

/// `XXH3_len_0to16_128b`.
#[inline]
pub(super) fn hash128_0to16(input: &[u8], secret: &Head, seed: u64) -> u128 {
    match input.len() {
        0 => {
            let low = avalanche_xxh64(seed ^ le64(secret, 64) ^ le64(secret, 72));
            joined(low, avalanche_xxh64(seed ^ le64(secret, 80) ^ le64(secret, 88)))
        }
        1..=3 => {
            let low = combined_1to3(input);
            let high = low.swap_bytes().rotate_left(13);
            let flip_low = u64::from(le32(secret, 0) ^ le32(secret, 4)).wrapping_add(seed);
            let flip_high = u64::from(le32(secret, 8) ^ le32(secret, 12)).wrapping_sub(seed);
            joined(avalanche_xxh64(u64::from(low) ^ flip_low), avalanche_xxh64(u64::from(high) ^ flip_high))
        }
        4..=8 => hash128_4to8(input, secret, seed),
        _ => hash128_9to16(input, secret, seed),
    }
}

/// `XXH3_len_4to8_128b`.
#[inline]
fn hash128_4to8(input: &[u8], secret: &Head, seed: u64) -> u128 {
    let (first, last) = ends32(input);
    let bitflip = (le64(secret, 16) ^ le64(secret, 24)).wrapping_add(seed_4to8(seed));
    let keyed = first.wrapping_add(last << 32_u32) ^ bitflip;
    let (mut low, mut high) = mul128(keyed, PRIME64_1.wrapping_add(len64(input.len()) << 2_u32));
    high = high.wrapping_add(low << 1_u32);
    low ^= high >> 3_u32;
    low ^= low >> 35_u32;
    low = low.wrapping_mul(PRIME_MX2);
    low ^= low >> 28_u32;
    joined(low, avalanche(high))
}

/// `XXH3_len_9to16_128b`, on a machine of 64-bit words: every machine's
/// result is the same.
#[inline]
fn hash128_9to16(input: &[u8], secret: &Head, seed: u64) -> u128 {
    let flip_low = (le64(secret, 32) ^ le64(secret, 40)).wrapping_sub(seed);
    let flip_high = (le64(secret, 48) ^ le64(secret, 56)).wrapping_add(seed);
    let (first, last) = ends64(input);
    let (mut low, mut high) = mul128(first ^ last ^ flip_low, PRIME64_1);
    low = low.wrapping_add(len64(input.len()).wrapping_sub(1) << 54_u32);
    let last = last ^ flip_high;
    let last_low = last & u64::from(u32::MAX);
    high = high.wrapping_add(last).wrapping_add(last_low.wrapping_mul(PRIME32_2.wrapping_sub(1)));
    low ^= high.swap_bytes();
    let (out_low, out_high) = mul128(low, PRIME64_2);
    joined(avalanche(out_low), avalanche(out_high.wrapping_add(high.wrapping_mul(PRIME64_2))))
}
