//! \[DESIGN\] XXH3 streamed (`xxhash.h`'s `XXH3_state_t`,
//! `XXH3_64bits_update`, `XXH3_64bits_digest`, `XXH3_128bits_digest`):
//! input given in pieces gives what it gives at once, at every split. A
//! buffer of four stripes holds what is not yet accumulated, and keeps the
//! last stripe accumulated, which the digest needs when fewer than 64
//! bytes follow it.
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

use super::long::{Stripe, accumulate_512, finish64, finish128, hash64, hash128, last_key, scramble_key, stripes_per_block};
use super::{
    ACC_NB, DEFAULT_SECRET, MIDSIZE_MAX, SECRET_CONSUME_RATE, SECRET_DEFAULT_SIZE, STRIPE_LEN, Scalar, Secret, Stripes, derive_secret,
    len64,
};

/// `XXH3_INTERNALBUFFER_SIZE`: the bytes the state holds, four stripes.
const BUFFER: usize = 256;

/// The secret a state hashes by.
#[derive(Clone)]
enum Keys<'a> {
    /// The default secret, or one given.
    Given(&'a [u8]),
    /// One derived from a seed (`derive_secret`).
    Derived([u8; SECRET_DEFAULT_SIZE]),
}

impl Keys<'_> {
    fn bytes(&self) -> &[u8] {
        match self {
            Keys::Given(bytes) => bytes,
            Keys::Derived(bytes) => bytes,
        }
    }
}

/// \[DESIGN\] XXH3 streamed: `update` with the input's pieces in order,
/// then `digest64` or `digest128`, which leave the state as it was, so
/// more may follow.
#[derive(Clone)]
pub struct Xxh3<'a> {
    acc: [u64; ACC_NB],
    buffer: [u8; BUFFER],
    buffered: usize,
    /// Stripes accumulated of the block under way.
    stripes_so_far: usize,
    /// Stripes in a block of the secret.
    per_block: usize,
    total: u64,
    /// The seed of inputs of up to 240 bytes, if one was given.
    seed: Option<u64>,
    keys: Keys<'a>,
}

impl Default for Xxh3<'_> {
    fn default() -> Self {
        Xxh3::new()
    }
}

impl<'a> Xxh3<'a> {
    /// The state of no input, seed 0 and the default secret:
    /// `XXH3_64bits_reset`.
    pub fn new() -> Xxh3<'a> {
        Xxh3::of(Keys::Given(&DEFAULT_SECRET), None)
    }

    /// The state of no input and `seed`: `XXH3_64bits_reset_withSeed`.
    pub fn with_seed(seed: u64) -> Xxh3<'a> {
        if seed == 0 { Xxh3::new() } else { Xxh3::of(Keys::Derived(derive_secret(seed)), Some(seed)) }
    }

    /// The state of no input and `secret`:
    /// `XXH3_64bits_reset_withSecret`.
    pub fn with_secret(secret: Secret<'a>) -> Xxh3<'a> {
        Xxh3::of(Keys::Given(secret.as_bytes()), None)
    }

    fn of(keys: Keys<'a>, seed: Option<u64>) -> Xxh3<'a> {
        let per_block = stripes_per_block(keys.bytes().len());
        let (buffer, acc) = ([0; BUFFER], super::long::INIT_ACC);
        Xxh3 { acc, buffer, buffered: 0, stripes_so_far: 0, per_block, total: 0, seed, keys }
    }

    /// The state with `input` added: `update` by value, to fold pieces.
    #[must_use]
    pub fn updated(mut self, input: &[u8]) -> Xxh3<'a> {
        self.update(input);
        self
    }

    /// Adds `input`, the next piece: `XXH3_64bits_update`.
    pub fn update(&mut self, input: &[u8]) {
        self.total = self.total.wrapping_add(len64(input.len()));
        let room = BUFFER.saturating_sub(self.buffered);
        if input.len() <= room {
            self.keep(input);
            return;
        }
        let Xxh3 { acc, buffer, buffered, stripes_so_far, per_block, keys, .. } = self;
        let secret = keys.bytes();
        let mut ahead = Ahead { acc, stripes_so_far, per_block: *per_block, secret };
        let mut rest = input;
        if *buffered > 0 {
            let (head, tail) = rest.split_at(room);
            if let Some(slot) = buffer.get_mut(*buffered..) {
                slot.copy_from_slice(head);
            }
            ahead.consume(buffer.as_chunks::<STRIPE_LEN>().0);
            *buffered = 0;
            rest = tail;
        }
        if rest.len() > BUFFER {
            let whole = rest.len().wrapping_sub(1).checked_div(STRIPE_LEN).unwrap_or(0);
            let (stripes, _) = rest.as_chunks::<STRIPE_LEN>();
            let taken = stripes.get(..whole).unwrap_or_default();
            ahead.consume(taken);
            if let (Some(last), Some(slot)) = (taken.last(), buffer.last_chunk_mut::<STRIPE_LEN>()) {
                *slot = *last;
            }
            rest = rest.get(whole.wrapping_mul(STRIPE_LEN)..).unwrap_or_default();
        }
        self.keep(rest);
    }

    /// Puts `input` after what the buffer holds; it has room for it.
    fn keep(&mut self, input: &[u8]) {
        let end = self.buffered.saturating_add(input.len());
        if let Some(slot) = self.buffer.get_mut(self.buffered..end) {
            slot.copy_from_slice(input);
            self.buffered = end;
        }
    }

    /// The input so far, if it is all in the buffer: of up to 240 bytes.
    fn short_input(&self) -> Option<&[u8]> {
        let total = usize::try_from(self.total).ok().filter(|&total| total <= MIDSIZE_MAX)?;
        self.buffer.get(..total)
    }

    /// `XXH3_digest_long`: the accumulators of the input so far, with the
    /// buffer's stripes but its last, then the input's last 64 bytes.
    fn digest_long(&self) -> [u64; ACC_NB] {
        let mut acc = self.acc;
        let mut stripes_so_far = self.stripes_so_far;
        let secret = self.keys.bytes();
        let mut last = [0; STRIPE_LEN];
        if self.buffered >= STRIPE_LEN {
            let before_last = self.buffered.wrapping_sub(1).checked_div(STRIPE_LEN).unwrap_or(0);
            let stripes = self.buffer.as_chunks::<STRIPE_LEN>().0.get(..before_last).unwrap_or_default();
            Ahead { acc: &mut acc, stripes_so_far: &mut stripes_so_far, per_block: self.per_block, secret }.consume(stripes);
            if let Some(tail) = self.buffer.get(..self.buffered).and_then(<[u8]>::last_chunk::<STRIPE_LEN>) {
                last = *tail;
            }
        } else {
            let catch_up = STRIPE_LEN.wrapping_sub(self.buffered);
            let (before, after) = last.split_at_mut(catch_up);
            before.copy_from_slice(self.buffer.get(BUFFER.wrapping_sub(catch_up)..).unwrap_or_default());
            after.copy_from_slice(self.buffer.get(..self.buffered).unwrap_or_default());
        }
        if let Some(key) = last_key(secret) {
            accumulate_512(&mut acc, &last, key);
        }
        acc
    }

    /// XXH3-64 of the input so far: `XXH3_64bits_digest`.
    pub fn digest64(&self) -> u64 {
        let secret = self.keys.bytes();
        match (self.short_input(), self.seed) {
            (Some(input), Some(seed)) => super::xxh3_64_with_seed(input, seed),
            (Some(input), None) => hash64(&Scalar, input, 0, secret),
            (None, _) => finish64(&self.digest_long(), secret, self.total),
        }
    }

    /// XXH3-128 of the input so far: `XXH3_128bits_digest`.
    pub fn digest128(&self) -> u128 {
        let secret = self.keys.bytes();
        match (self.short_input(), self.seed) {
            (Some(input), Some(seed)) => super::xxh3_128_with_seed(input, seed),
            (Some(input), None) => hash128(&Scalar, input, 0, secret),
            (None, _) => finish128(&self.digest_long(), secret, self.total),
        }
    }
}

/// The accumulation of whole stripes, through blocks:
/// `XXH3_consumeStripes`.
struct Ahead<'s> {
    acc: &'s mut [u64; ACC_NB],
    stripes_so_far: &'s mut usize,
    per_block: usize,
    secret: &'s [u8],
}

impl Ahead<'_> {
    /// Accumulates `stripes`, scrambling at the end of each block.
    fn consume(&mut self, mut stripes: &[Stripe]) {
        let Some(scrambler) = scramble_key(self.secret) else { return };
        let mut from = self.stripes_so_far.wrapping_mul(SECRET_CONSUME_RATE);
        let mut this_block = self.per_block.saturating_sub(*self.stripes_so_far);
        if stripes.len() >= this_block {
            loop {
                let (now, rest) = stripes.split_at(this_block.min(stripes.len()));
                Scalar.accumulate(self.acc, now, self.secret.get(from..).unwrap_or_default());
                Scalar.scramble(self.acc, scrambler);
                (stripes, this_block, from) = (rest, self.per_block, 0);
                if stripes.len() < self.per_block {
                    break;
                }
            }
            *self.stripes_so_far = 0;
        }
        if !stripes.is_empty() {
            Scalar.accumulate(self.acc, stripes, self.secret.get(from..).unwrap_or_default());
            *self.stripes_so_far = self.stripes_so_far.saturating_add(stripes.len());
        }
    }
}
