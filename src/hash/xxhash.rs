//! \[DESIGN\] xxHash (RULES.md D30, D460): Yann Collet's non-cryptographic
//! hash functions, ported from the reference C implementation
//! (`xxhash.h`, version 0.8.3) and its specification
//! (`doc/xxhash_spec.md`, version 0.2.0). Each function gives exactly the
//! reference's value for every input, seed and secret, on any machine:
//! XXH64 (`xxh64`, `Xxh64`), XXH3-64 and XXH3-128 (`xxh3_64*`,
//! `xxh3_128*`, `Xxh3`), each seeded or of a secret, at once or streamed.
//!
//! XXH3 reads inputs of up to 240 bytes by a few multiplications of the
//! input's words, and longer ones by stripes of 64 bytes, eight lanes
//! wide, into eight accumulators. That loop is the one wider instructions
//! speed up, so it sits behind `Stripes`: `Scalar` here, and a kernel
//! crate's vector loops through the database's `Kernels` (RULES.md D354).
//!
//! As hash functions for files, `Xxh3Hash` (`XXH3_64`) and `Xxh128Hash`
//! (`XXH3_128`) give xxHash's canonical form, big-endian, the bytes
//! `xxhsum` prints.
//!
//! The port keeps the reference's notice, as its license asks:
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

use super::{Digest, HashFunction, HashId};

mod hasher;
mod long;
mod mid;
mod short;
mod stream;
mod xxh64;

pub use hasher::{Xxh3BuildHasher, Xxh3Hasher, Xxh3Map, Xxh3Set};
pub use long::{Scalar, Stripe, Stripes, accumulate_512, derive_secret, keyed};
pub use stream::Xxh3;
pub use xxh64::{Xxh64, xxh64};

/// The id of `Xxh3Hash` in file formats.
pub const XXH3_64: HashId = HashId(14);
/// The id of `Xxh128Hash` in file formats.
pub const XXH3_128: HashId = HashId(15);

// The constants of the specification's "Overview" sections and of
// `xxhash.h`, under their names there.

/// `XXH_PRIME32_1`.
const PRIME32_1: u64 = 0x9E37_79B1;
/// `XXH_PRIME32_2`.
const PRIME32_2: u64 = 0x85EB_CA77;
/// `XXH_PRIME32_3`.
const PRIME32_3: u64 = 0xC2B2_AE3D;
/// `XXH_PRIME64_1`.
const PRIME64_1: u64 = 0x9E37_79B1_85EB_CA87;
/// `XXH_PRIME64_2`.
const PRIME64_2: u64 = 0xC2B2_AE3D_27D4_EB4F;
/// `XXH_PRIME64_3`.
const PRIME64_3: u64 = 0x1656_67B1_9E37_79F9;
/// `XXH_PRIME64_4`.
const PRIME64_4: u64 = 0x85EB_CA77_C2B2_AE63;
/// `XXH_PRIME64_5`.
const PRIME64_5: u64 = 0x27D4_EB2F_1656_67C5;
/// `PRIME_MX1`, of `XXH3_avalanche`.
const PRIME_MX1: u64 = 0x1656_6791_9E37_79F9;
/// `PRIME_MX2`, of `XXH3_rrmxmx` and the 128-bit 4 to 8 bytes.
const PRIME_MX2: u64 = 0x9FB2_1C65_1E98_DF25;

/// `XXH3_SECRET_SIZE_MIN`: the shortest secret XXH3 takes.
pub const SECRET_SIZE_MIN: usize = 136;
/// `XXH_SECRET_DEFAULT_SIZE`: the bytes of the default secret.
pub const SECRET_DEFAULT_SIZE: usize = 192;
/// `XXH3_MIDSIZE_MAX`: the longest input read without stripes.
const MIDSIZE_MAX: usize = 240;
/// `XXH_STRIPE_LEN`: the bytes of a stripe.
pub const STRIPE_LEN: usize = 64;
/// `XXH_ACC_NB`: the accumulators of a long input, one a lane.
pub const ACC_NB: usize = 8;
/// `XXH_SECRET_CONSUME_RATE`: how far the secret moves for each stripe.
pub const SECRET_CONSUME_RATE: usize = 8;

/// `XXH3_kSecret`: the default secret, the specification's
/// `defaultSecret`.
pub const DEFAULT_SECRET: [u8; SECRET_DEFAULT_SIZE] = [
    0xb8, 0xfe, 0x6c, 0x39, 0x23, 0xa4, 0x4b, 0xbe, 0x7c, 0x01, 0x81, 0x2c, 0xf7, 0x21, 0xad, 0x1c, //
    0xde, 0xd4, 0x6d, 0xe9, 0x83, 0x90, 0x97, 0xdb, 0x72, 0x40, 0xa4, 0xa4, 0xb7, 0xb3, 0x67, 0x1f, //
    0xcb, 0x79, 0xe6, 0x4e, 0xcc, 0xc0, 0xe5, 0x78, 0x82, 0x5a, 0xd0, 0x7d, 0xcc, 0xff, 0x72, 0x21, //
    0xb8, 0x08, 0x46, 0x74, 0xf7, 0x43, 0x24, 0x8e, 0xe0, 0x35, 0x90, 0xe6, 0x81, 0x3a, 0x26, 0x4c, //
    0x3c, 0x28, 0x52, 0xbb, 0x91, 0xc3, 0x00, 0xcb, 0x88, 0xd0, 0x65, 0x8b, 0x1b, 0x53, 0x2e, 0xa3, //
    0x71, 0x64, 0x48, 0x97, 0xa2, 0x0d, 0xf9, 0x4e, 0x38, 0x19, 0xef, 0x46, 0xa9, 0xde, 0xac, 0xd8, //
    0xa8, 0xfa, 0x76, 0x3f, 0xe3, 0x9c, 0x34, 0x3f, 0xf9, 0xdc, 0xbb, 0xc7, 0xc7, 0x0b, 0x4f, 0x1d, //
    0x8a, 0x51, 0xe0, 0x4b, 0xcd, 0xb4, 0x59, 0x31, 0xc8, 0x9f, 0x7e, 0xc9, 0xd9, 0x78, 0x73, 0x64, //
    0xea, 0xc5, 0xac, 0x83, 0x34, 0xd3, 0xeb, 0xc3, 0xc5, 0x81, 0xa0, 0xff, 0xfa, 0x13, 0x63, 0xeb, //
    0x17, 0x0d, 0xdd, 0x51, 0xb7, 0xf0, 0xda, 0x49, 0xd3, 0x16, 0x55, 0x26, 0x29, 0xd4, 0x68, 0x9e, //
    0x2b, 0x16, 0xbe, 0x58, 0x7d, 0x47, 0xa1, 0xfc, 0x8f, 0xf8, 0xb8, 0xd1, 0x7a, 0xd0, 0x31, 0xce, //
    0x45, 0xcb, 0x3a, 0x8f, 0x95, 0x16, 0x04, 0x28, 0xaf, 0xd7, 0xfb, 0xca, 0xbb, 0x4b, 0x40, 0x7e, //
];

/// A secret XXH3 hashes by, at least `SECRET_SIZE_MIN` bytes: its whole
/// length is read by long inputs, its first `SECRET_SIZE_MIN` by short
/// ones.
#[derive(Debug, Clone, Copy)]
pub struct Secret<'a>(&'a [u8]);

impl<'a> Secret<'a> {
    /// The secret of `bytes`; `None` if they are fewer than
    /// `SECRET_SIZE_MIN`, as the reference refuses them.
    pub fn new(bytes: &'a [u8]) -> Option<Secret<'a>> {
        (bytes.len() >= SECRET_SIZE_MIN).then_some(Secret(bytes))
    }

    /// Its bytes.
    pub fn as_bytes(&self) -> &'a [u8] {
        self.0
    }
}

/// The little-endian word at `at` of `bytes`, 0 past their end: callers
/// read only within the input's length and the secret's
/// `SECRET_SIZE_MIN`, so the 0 is never taken.
#[inline]
fn le64(bytes: &[u8], at: usize) -> u64 {
    bytes.get(at..).and_then(<[u8]>::first_chunk::<8>).map_or(0, |b| u64::from_le_bytes(*b))
}

/// The little-endian 32-bit word at `at` of `bytes`, 0 past their end.
#[inline]
fn le32(bytes: &[u8], at: usize) -> u32 {
    bytes.get(at..).and_then(<[u8]>::first_chunk::<4>).map_or(0, |b| u32::from_le_bytes(*b))
}

/// A length as the functions add it in: a `usize` always fits 64 bits.
#[inline]
fn len64(len: usize) -> u64 {
    u64::try_from(len).unwrap_or(u64::MAX)
}

/// The low and high 64-bit halves of `p`.
#[inline]
fn halves(p: u128) -> (u64, u64) {
    (u64::try_from(p & u128::from(u64::MAX)).unwrap_or(0), u64::try_from(p >> 64_u32).unwrap_or(0))
}

/// `XXH_mult64to128`: the 128-bit product of `a` and `b`, as its halves.
#[inline]
fn mul128(a: u64, b: u64) -> (u64, u64) {
    halves(u128::from(a).wrapping_mul(u128::from(b)))
}

/// `XXH3_mul128_fold64`: the halves of the 128-bit product xored.
#[inline]
fn mul128_fold64(a: u64, b: u64) -> u64 {
    let (low, high) = mul128(a, b);
    low ^ high
}

/// `XXH64_avalanche`, the specification's `avalanche_XXH64`.
#[inline]
fn avalanche_xxh64(mut h: u64) -> u64 {
    h ^= h >> 33_u32;
    h = h.wrapping_mul(PRIME64_2);
    h ^= h >> 29_u32;
    h = h.wrapping_mul(PRIME64_3);
    h ^ (h >> 32_u32)
}

/// `XXH3_avalanche`, the specification's `avalanche`.
#[inline]
fn avalanche(mut h: u64) -> u64 {
    h ^= h >> 37_u32;
    h = h.wrapping_mul(PRIME_MX1);
    h ^ (h >> 32_u32)
}

/// `XXH3_rrmxmx`: the final mix of XXH3-64's 4 to 8 bytes.
#[inline]
fn rrmxmx(mut h: u64, len: u64) -> u64 {
    h ^= h.rotate_left(49) ^ h.rotate_left(24);
    h = h.wrapping_mul(PRIME_MX2);
    h ^= (h >> 35_u32).wrapping_add(len);
    h = h.wrapping_mul(PRIME_MX2);
    h ^ (h >> 28_u32)
}

/// The value of a 128-bit hash of its `low` and `high` halves, as
/// `XXH128_hash_t` holds them.
#[inline]
fn joined(low: u64, high: u64) -> u128 {
    (u128::from(high) << 64_u32) | u128::from(low)
}

/// XXH3-64 of `input`, of seed 0 and the default secret:
/// `XXH3_64bits`.
pub fn xxh3_64(input: &[u8]) -> u64 {
    xxh3_64_on(&Scalar, input, 0)
}

/// XXH3-64 of `input` and `seed`: `XXH3_64bits_withSeed`.
pub fn xxh3_64_with_seed(input: &[u8], seed: u64) -> u64 {
    xxh3_64_on(&Scalar, input, seed)
}

/// XXH3-64 of `input` by `secret`: `XXH3_64bits_withSecret`.
pub fn xxh3_64_with_secret(input: &[u8], secret: Secret<'_>) -> u64 {
    long::hash64(&Scalar, input, 0, secret.0)
}

/// XXH3-64 of `input` and `seed`, its long inputs' stripes by `stripes`.
pub fn xxh3_64_on<S: Stripes + ?Sized>(stripes: &S, input: &[u8], seed: u64) -> u64 {
    if input.len() <= MIDSIZE_MAX || seed == 0 {
        return long::hash64(stripes, input, seed, &DEFAULT_SECRET);
    }
    long::hash64(stripes, input, seed, &derive_secret(seed))
}

/// XXH3-128 of `input`, of seed 0 and the default secret:
/// `XXH3_128bits`, its high half in the high bits.
pub fn xxh3_128(input: &[u8]) -> u128 {
    xxh3_128_on(&Scalar, input, 0)
}

/// XXH3-128 of `input` and `seed`: `XXH3_128bits_withSeed`.
pub fn xxh3_128_with_seed(input: &[u8], seed: u64) -> u128 {
    xxh3_128_on(&Scalar, input, seed)
}

/// XXH3-64 of `words`, each little-endian, by `seed`: a key of a few
/// numbers hashed as their bytes, the same on every machine.
pub fn xxh3_64_words(words: &[u64], seed: u64) -> u64 {
    use std::hash::Hasher as _;
    let mut hasher = Xxh3Hasher::with_seed(seed);
    for &word in words {
        hasher.write_u64(word);
    }
    hasher.finish()
}

/// XXH3-128 of `input` and `seed`, as its low and high halves: two
/// hashes of one input, as a Bloom filter's probes take them.
pub fn xxh3_128_halves(input: &[u8], seed: u64) -> (u64, u64) {
    halves(xxh3_128_with_seed(input, seed))
}

/// XXH3-128 of `input` by `secret`: `XXH3_128bits_withSecret`.
pub fn xxh3_128_with_secret(input: &[u8], secret: Secret<'_>) -> u128 {
    long::hash128(&Scalar, input, 0, secret.0)
}

/// XXH3-128 of `input` and `seed`, its long inputs' stripes by `stripes`.
pub fn xxh3_128_on<S: Stripes + ?Sized>(stripes: &S, input: &[u8], seed: u64) -> u128 {
    if input.len() <= MIDSIZE_MAX || seed == 0 {
        return long::hash128(stripes, input, seed, &DEFAULT_SECRET);
    }
    long::hash128(stripes, input, seed, &derive_secret(seed))
}

/// \[DESIGN\] XXH3-64 of words by one seed: each word's value is
/// `xxh3_64_with_seed` of its 8 bytes, little-endian, with what depends
/// on the seed alone computed once. A table of groups hashes its integer
/// keys so (RULES.md D357, D460).
#[derive(Debug, Clone, Copy)]
pub struct Words {
    /// The 4 to 8 bytes' `bitflip` of the seed.
    bitflip: u64,
    /// The 9 to 16 bytes' `bitflip1` and `bitflip2` of the seed.
    pair_flips: (u64, u64),
}

/// Bytes of a word, as `Words` hashes it.
const WORD_LEN: u64 = 8;

impl Words {
    /// Words hashed by `seed`.
    pub fn new(seed: u64) -> Words {
        Words { bitflip: short::bitflip_4to8(&DEFAULT_SECRET, seed), pair_flips: short::bitflips_9to16(&DEFAULT_SECRET, seed) }
    }

    /// The hash of `word`: of its little-endian bytes, whose first four
    /// are its low half and last four its high, so the input the 4 to 8
    /// bytes combine is the word turned by half.
    #[inline]
    pub fn hash(&self, word: u64) -> u64 {
        rrmxmx(word.rotate_left(32) ^ self.bitflip, WORD_LEN)
    }

    /// The hash of the words `first` and `second`: of their 16 bytes,
    /// little-endian, the first word first, as a key of two numbers (a
    /// page of a file) is hashed.
    #[inline]
    pub fn pair(&self, first: u64, second: u64) -> u64 {
        short::hash64_9to16_words((first, second), self.pair_flips, 2 * WORD_LEN)
    }
}

/// \[DESIGN\] XXH3-64, seed 0 and the default secret, as a
/// `HashFunction`: 8 bytes, big-endian, xxHash's canonical form.
pub struct Xxh3Hash;

/// The name of `Xxh3Hash`, as STORAGE.CHECKSUM names it.
pub const XXH3_64_NAME: &str = "XXH3_64";

impl HashFunction for Xxh3Hash {
    fn id(&self) -> HashId {
        XXH3_64
    }
    fn name(&self) -> &'static str {
        XXH3_64_NAME
    }
    fn digest_len(&self) -> usize {
        size_of::<u64>()
    }
    fn hash_parts(&self, parts: &[&[u8]]) -> Digest {
        let value = match parts {
            [one] => xxh3_64(one),
            _ => parts.iter().fold(Xxh3::new(), |state, part| state.updated(part)).digest64(),
        };
        Digest::new(&value.to_be_bytes()).unwrap_or_default()
    }
}

/// \[DESIGN\] XXH3-128, seed 0 and the default secret, as a
/// `HashFunction`: 16 bytes, big-endian, its high half first, xxHash's
/// canonical form.
pub struct Xxh128Hash;

impl HashFunction for Xxh128Hash {
    fn id(&self) -> HashId {
        XXH3_128
    }
    fn name(&self) -> &'static str {
        "XXH3_128"
    }
    fn digest_len(&self) -> usize {
        size_of::<u128>()
    }
    fn hash_parts(&self, parts: &[&[u8]]) -> Digest {
        let value = match parts {
            [one] => xxh3_128(one),
            _ => parts.iter().fold(Xxh3::new(), |state, part| state.updated(part)).digest128(),
        };
        Digest::new(&value.to_be_bytes()).unwrap_or_default()
    }
}

// Unit tests: the reference's published sanity vectors, streaming against
// one-shot at every split, and the hash functions' contract (RULES.md
// R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
