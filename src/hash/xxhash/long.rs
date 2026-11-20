//! \[DESIGN\] XXH3 of inputs longer than 240 bytes (`xxhash.h`'s
//! `XXH3_hashLong_*`; the specification's "large inputs"): stripes of 64
//! bytes accumulated in eight lanes, the accumulators scrambled after each
//! block, then merged. The stripes' two loops are `Stripes`, so wider
//! instructions can run them; `Scalar` is the reference's scalar loop,
//! written so the compiler sees no bound to check within a stripe.
//!
//! Every length dispatches here (`hash64`, `hash128`), as the reference's
//! `XXH3_64bits_internal` does, so one function holds the choice by length.
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

use super::mid;
use super::short::{self, Head};
use super::{
    ACC_NB, DEFAULT_SECRET, MIDSIZE_MAX, PRIME32_1, PRIME32_2, PRIME32_3, PRIME64_1, PRIME64_2, PRIME64_3, PRIME64_4, PRIME64_5,
    SECRET_CONSUME_RATE, SECRET_DEFAULT_SIZE, SECRET_SIZE_MIN, STRIPE_LEN, avalanche, joined, le64, len64, mul128_fold64,
};

/// A stripe's bytes.
pub type Stripe = [u8; STRIPE_LEN];

/// `XXH_SECRET_LASTACC_START`: how far before the secret's last stripe
/// the last stripe of the input is keyed.
const SECRET_LASTACC_START: usize = 7;
/// `XXH_SECRET_MERGEACCS_START`: where the 64-bit merge's key starts.
const SECRET_MERGEACCS_START: usize = 11;
/// Shortest inputs the up to 16 bytes' path reads.
const SHORT_MAX: usize = 16;
/// Shortest inputs the 17 to 128 bytes' path reads.
const MID_SHORT_MAX: usize = 128;
/// The shift of `XXH3_scalarScrambleRound`.
const SCRAMBLE_SHIFT: u32 = 47;

/// `XXH3_INIT_ACC`: the accumulators before the first stripe.
pub(super) const INIT_ACC: [u64; ACC_NB] = [PRIME32_3, PRIME64_1, PRIME64_2, PRIME64_3, PRIME64_4, PRIME32_2, PRIME64_5, PRIME32_1];

/// \[DESIGN\] XXH3's two loops over long inputs, which wider instructions
/// may run: each implementation gives exactly what `Scalar` gives.
pub trait Stripes {
    /// `XXH3_accumulate`: accumulates each of `stripes` into `acc`, the
    /// `n`th keyed by the 64 bytes of `secret` at `n` times
    /// `SECRET_CONSUME_RATE`. `secret` holds them all.
    fn accumulate(&self, acc: &mut [u64; ACC_NB], stripes: &[Stripe], secret: &[u8]);
    /// `XXH3_scrambleAcc`: scrambles `acc` by `key`, the secret's last
    /// 64 bytes.
    fn scramble(&self, acc: &mut [u64; ACC_NB], key: &Stripe);
}

/// \[DESIGN\] The reference's scalar loops (`XXH3_scalarRound`,
/// `XXH3_scalarScrambleRound`), the portable `Stripes`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Scalar;

impl Stripes for Scalar {
    #[inline]
    fn accumulate(&self, acc: &mut [u64; ACC_NB], stripes: &[Stripe], secret: &[u8]) {
        let keys = secret.windows(STRIPE_LEN).step_by(SECRET_CONSUME_RATE).filter_map(<[u8]>::first_chunk::<STRIPE_LEN>);
        for (stripe, key) in stripes.iter().zip(keys) {
            accumulate_512(acc, stripe, key);
        }
    }

    #[inline]
    fn scramble(&self, acc: &mut [u64; ACC_NB], key: &Stripe) {
        let (keys, _) = key.as_chunks::<8>();
        for (a, k) in acc.iter_mut().zip(keys) {
            let mixed = (*a ^ (*a >> SCRAMBLE_SHIFT)) ^ u64::from_le_bytes(*k);
            *a = mixed.wrapping_mul(PRIME32_1);
        }
    }
}

/// The stripes of `stripes` whose keys `secret` holds: every one, of a
/// caller keeping `Stripes::accumulate`'s contract.
pub fn keyed<'a>(stripes: &'a [Stripe], secret: &[u8]) -> &'a [Stripe] {
    let keys = secret.len().checked_sub(STRIPE_LEN).and_then(|n| n.checked_div(SECRET_CONSUME_RATE)).map_or(0, |n| n.saturating_add(1));
    stripes.get(..keys.min(stripes.len())).unwrap_or_default()
}

/// `XXH3_accumulate_512`, scalar: each lane's input and key multiplied by
/// halves into its accumulator, its input added to its neighbour's. Lanes
/// go by pairs, so each pair's swap is two names, not an index.
#[inline]
pub fn accumulate_512(acc: &mut [u64; ACC_NB], stripe: &Stripe, key: &Stripe) {
    let (pairs, _) = acc.as_chunks_mut::<2>();
    let (inputs, _) = stripe.as_chunks::<16>();
    let (keys, _) = key.as_chunks::<16>();
    for ((a, input), pair_key) in pairs.iter_mut().zip(inputs).zip(keys) {
        let (low_in, high_in) = split16(input);
        let (low_key, high_key) = split16(pair_key);
        let (low, high) = (low_in ^ low_key, high_in ^ high_key);
        a[0] = a[0].wrapping_add(high_in).wrapping_add((low & u64::from(u32::MAX)).wrapping_mul(low >> 32_u32));
        a[1] = a[1].wrapping_add(low_in).wrapping_add((high & u64::from(u32::MAX)).wrapping_mul(high >> 32_u32));
    }
}

/// The two little-endian words of 16 bytes.
#[inline]
fn split16(bytes: &[u8; 16]) -> (u64, u64) {
    let (words, _) = bytes.as_chunks::<8>();
    let mut out = [0_u64; 2];
    out.iter_mut().zip(words).for_each(|(w, b)| *w = u64::from_le_bytes(*b));
    let [low, high] = out;
    (low, high)
}

/// `XXH3_initCustomSecret`: the default secret with `seed` added to each
/// even word and taken from each odd one, the secret of long inputs of a
/// seed.
pub fn derive_secret(seed: u64) -> [u8; SECRET_DEFAULT_SIZE] {
    let mut secret = DEFAULT_SECRET;
    let (pairs, _) = secret.as_chunks_mut::<16>();
    for pair in pairs {
        let (low, high) = split16(pair);
        let (first, second) = pair.split_at_mut(8);
        first.copy_from_slice(&low.wrapping_add(seed).to_le_bytes());
        second.copy_from_slice(&high.wrapping_sub(seed).to_le_bytes());
    }
    secret
}

/// The secret's first stripe from its end, the key of `scramble`; `None`
/// of a secret shorter than `SECRET_SIZE_MIN`.
#[inline]
pub(super) fn scramble_key(secret: &[u8]) -> Option<&Stripe> {
    secret.last_chunk::<STRIPE_LEN>()
}

/// The key of the last stripe of an input.
#[inline]
pub(super) fn last_key(secret: &[u8]) -> Option<&Stripe> {
    let at = secret.len().checked_sub(STRIPE_LEN)?.checked_sub(SECRET_LASTACC_START)?;
    secret.get(at..)?.first_chunk::<STRIPE_LEN>()
}

/// Stripes in a block of a secret of `len` bytes.
#[inline]
pub(super) fn stripes_per_block(len: usize) -> usize {
    len.saturating_sub(STRIPE_LEN).checked_div(SECRET_CONSUME_RATE).unwrap_or(0)
}

/// `XXH3_hashLong_internal_loop`: the accumulators of the long `input`,
/// every block but the last accumulated and scrambled, then the last
/// block's stripes but its last, then the input's last 64 bytes.
#[inline]
fn long_loop<S: Stripes + ?Sized>(stripes: &S, input: &[u8], secret: &[u8]) -> [u64; ACC_NB] {
    let mut acc = INIT_ACC;
    let (Some(scrambler), Some(last)) = (scramble_key(secret), last_key(secret)) else { return acc };
    let block_len = stripes_per_block(secret.len()).wrapping_mul(STRIPE_LEN);
    let blocks = input.len().wrapping_sub(1).checked_div(block_len).unwrap_or(0);
    let (whole, rest) = input.split_at(blocks.wrapping_mul(block_len).min(input.len()));
    for block in whole.chunks_exact(block_len.max(1)) {
        stripes.accumulate(&mut acc, block.as_chunks::<STRIPE_LEN>().0, secret);
        stripes.scramble(&mut acc, scrambler);
    }
    let (rest_stripes, _) = rest.as_chunks::<STRIPE_LEN>();
    let before_last = rest.len().wrapping_sub(1).checked_div(STRIPE_LEN).unwrap_or(0);
    stripes.accumulate(&mut acc, rest_stripes.get(..before_last).unwrap_or_default(), secret);
    if let Some(final_stripe) = input.last_chunk::<STRIPE_LEN>() {
        accumulate_512(&mut acc, final_stripe, last);
    }
    acc
}

/// `XXH3_mergeAccs`: the accumulators merged from `start`, by the 64
/// bytes of `secret` at `at`.
#[inline]
pub(super) fn merge(acc: &[u64; ACC_NB], secret: &[u8], at: usize, start: u64) -> u64 {
    let (pairs, _) = acc.as_chunks::<2>();
    let mut result = start;
    for (n, pair) in pairs.iter().enumerate() {
        let key = at.wrapping_add(n.wrapping_mul(16));
        result = result.wrapping_add(mul128_fold64(pair[0] ^ le64(secret, key), pair[1] ^ le64(secret, key.wrapping_add(8))));
    }
    avalanche(result)
}

/// `XXH3_finalizeLong_64b`.
#[inline]
pub(super) fn finish64(acc: &[u64; ACC_NB], secret: &[u8], len: u64) -> u64 {
    merge(acc, secret, SECRET_MERGEACCS_START, len.wrapping_mul(PRIME64_1))
}

/// `XXH3_finalizeLong_128b`.
#[inline]
pub(super) fn finish128(acc: &[u64; ACC_NB], secret: &[u8], len: u64) -> u128 {
    let at = secret.len().wrapping_sub(STRIPE_LEN).wrapping_sub(SECRET_MERGEACCS_START);
    joined(finish64(acc, secret, len), merge(acc, secret, at, !len.wrapping_mul(PRIME64_2)))
}

/// `XXH3_hashLong_64b_internal`: kept out of the short inputs' function,
/// as the reference keeps it, so their calls pay for none of its registers.
#[inline(never)]
fn hash64_long<S: Stripes + ?Sized>(stripes: &S, input: &[u8], secret: &[u8]) -> u64 {
    finish64(&long_loop(stripes, input, secret), secret, len64(input.len()))
}

/// `XXH3_hashLong_128b_internal`, kept apart as `hash64_long`.
#[inline(never)]
fn hash128_long<S: Stripes + ?Sized>(stripes: &S, input: &[u8], secret: &[u8]) -> u128 {
    finish128(&long_loop(stripes, input, secret), secret, len64(input.len()))
}

/// The default secret's first `SECRET_SIZE_MIN` bytes.
const DEFAULT_HEAD: &Head = match DEFAULT_SECRET.first_chunk::<SECRET_SIZE_MIN>() {
    Some(head) => head,
    None => &[0; SECRET_SIZE_MIN],
};

/// The secret's first `SECRET_SIZE_MIN` bytes, those short inputs read: the
/// default's of a shorter secret, which no caller gives (`Secret` refuses
/// one), so no input is hashed past its secret.
#[inline]
fn head(secret: &[u8]) -> &Head {
    secret.first_chunk::<SECRET_SIZE_MIN>().unwrap_or(DEFAULT_HEAD)
}

/// XXH3-64 of at most 128 bytes, seed `seed` and the default secret: the
/// short and mid inputs' paths called straight, with no secret to choose
/// and no long input to rule out, as a hash map's key wants. Past 128
/// bytes, the general path.
#[inline]
pub(super) fn hash64_short_key(input: &[u8], seed: u64) -> u64 {
    match input.len() {
        0..=SHORT_MAX => short::hash64_0to16(input, DEFAULT_HEAD, seed),
        17..=MID_SHORT_MAX => mid::hash64_17to128(input, DEFAULT_HEAD, seed),
        _ => super::xxh3_64_with_seed(input, seed),
    }
}

/// `XXH3_64bits_internal`: XXH3-64 of `input`, `seed` and `secret`. The
/// seed changes only inputs of up to 240 bytes, as the reference's: a long
/// one's seed is in its secret.
pub(super) fn hash64<S: Stripes + ?Sized>(stripes: &S, input: &[u8], seed: u64, secret: &[u8]) -> u64 {
    let head = head(secret);
    match input.len() {
        0..=SHORT_MAX => short::hash64_0to16(input, head, seed),
        17..=MID_SHORT_MAX => mid::hash64_17to128(input, head, seed),
        129..=MIDSIZE_MAX => mid::hash64_129to240(input, head, seed),
        _ => hash64_long(stripes, input, secret),
    }
}

/// `XXH3_128bits_internal`: XXH3-128 of `input`, `seed` and `secret`, as
/// `hash64`.
pub(super) fn hash128<S: Stripes + ?Sized>(stripes: &S, input: &[u8], seed: u64, secret: &[u8]) -> u128 {
    let head = head(secret);
    match input.len() {
        0..=SHORT_MAX => short::hash128_0to16(input, head, seed),
        17..=MID_SHORT_MAX => mid::hash128_17to128(input, head, seed),
        129..=MIDSIZE_MAX => mid::hash128_129to240(input, head, seed),
        _ => hash128_long(stripes, input, secret),
    }
}
