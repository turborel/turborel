//! \[DESIGN\] XXH64 (`xxhash.h`'s `XXH64`, `XXH64_update`,
//! `XXH64_digest`; the specification's "XXH64 Algorithm Description"):
//! stripes of 32 bytes into four accumulators, merged, then the input's
//! last bytes, at once or streamed.
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

use super::{PRIME64_1, PRIME64_2, PRIME64_3, PRIME64_4, PRIME64_5, avalanche_xxh64, le32, len64};

/// Bytes of a stripe.
const STRIPE: usize = 32;
/// The turns of the convergence of the four accumulators.
const MERGE_TURNS: [u32; 4] = [1, 7, 12, 18];

/// `XXH64_round`: one lane into its accumulator.
#[inline]
fn round(acc: u64, lane: u64) -> u64 {
    acc.wrapping_add(lane.wrapping_mul(PRIME64_2)).rotate_left(31).wrapping_mul(PRIME64_1)
}

/// `XXH64_mergeRound`.
#[inline]
fn merge_round(acc: u64, value: u64) -> u64 {
    (acc ^ round(0, value)).wrapping_mul(PRIME64_1).wrapping_add(PRIME64_4)
}

/// The accumulators before the first stripe of `seed`.
fn start(seed: u64) -> [u64; 4] {
    [seed.wrapping_add(PRIME64_1).wrapping_add(PRIME64_2), seed.wrapping_add(PRIME64_2), seed, seed.wrapping_sub(PRIME64_1)]
}

/// Accumulates each of `stripes`.
#[inline]
fn stripes(acc: &mut [u64; 4], stripes: &[[u8; STRIPE]]) {
    for stripe in stripes {
        let (lanes, _) = stripe.as_chunks::<8>();
        for (a, lane) in acc.iter_mut().zip(lanes) {
            *a = round(*a, u64::from_le_bytes(*lane));
        }
    }
}

/// The accumulators converged and merged.
fn converged(acc: &[u64; 4]) -> u64 {
    let turned = acc.iter().zip(MERGE_TURNS).fold(0_u64, |h, (a, turn)| h.wrapping_add(a.rotate_left(turn)));
    acc.iter().fold(turned, |h, &a| merge_round(h, a))
}

/// `XXH64_finalize`: the last `tail` bytes, fewer than a stripe, into `h`,
/// then the avalanche.
fn finish(mut h: u64, tail: &[u8]) -> u64 {
    let (words, rest) = tail.as_chunks::<8>();
    for word in words {
        h ^= round(0, u64::from_le_bytes(*word));
        h = h.rotate_left(27).wrapping_mul(PRIME64_1).wrapping_add(PRIME64_4);
    }
    let bytes = if rest.len() >= 4 {
        h ^= u64::from(le32(rest, 0)).wrapping_mul(PRIME64_1);
        h = h.rotate_left(23).wrapping_mul(PRIME64_2).wrapping_add(PRIME64_3);
        rest.get(4..).unwrap_or_default()
    } else {
        rest
    };
    for &byte in bytes {
        h ^= u64::from(byte).wrapping_mul(PRIME64_5);
        h = h.rotate_left(11).wrapping_mul(PRIME64_1);
    }
    avalanche_xxh64(h)
}

/// XXH64 of `input` and `seed`: `XXH64`.
pub fn xxh64(input: &[u8], seed: u64) -> u64 {
    let (whole, tail) = input.as_chunks::<STRIPE>();
    let h = if whole.is_empty() {
        seed.wrapping_add(PRIME64_5)
    } else {
        let mut acc = start(seed);
        stripes(&mut acc, whole);
        converged(&acc)
    };
    finish(h.wrapping_add(len64(input.len())), tail)
}

/// \[DESIGN\] XXH64 streamed: `update` with the input's pieces in order,
/// then `digest`, which leaves the state as it was.
#[derive(Debug, Clone)]
pub struct Xxh64 {
    acc: [u64; 4],
    seed: u64,
    /// Bytes of an unfinished stripe.
    pending: [u8; STRIPE],
    filled: usize,
    total: u64,
}

impl Xxh64 {
    /// The state of no input and `seed`: `XXH64_reset`.
    pub fn new(seed: u64) -> Xxh64 {
        Xxh64 { acc: start(seed), seed, pending: [0; STRIPE], filled: 0, total: 0 }
    }

    /// Adds `input`, the next piece: `XXH64_update`.
    pub fn update(&mut self, input: &[u8]) {
        self.total = self.total.wrapping_add(len64(input.len()));
        let mut rest = input;
        if self.filled > 0 {
            let wanted = STRIPE.saturating_sub(self.filled).min(rest.len());
            let (head, tail) = rest.split_at(wanted);
            if let Some(slot) = self.pending.get_mut(self.filled..self.filled.saturating_add(wanted)) {
                slot.copy_from_slice(head);
            }
            self.filled = self.filled.saturating_add(wanted);
            rest = tail;
            if self.filled < STRIPE {
                return;
            }
            stripes(&mut self.acc, &[self.pending]);
            self.filled = 0;
        }
        let (whole, tail) = rest.as_chunks::<STRIPE>();
        stripes(&mut self.acc, whole);
        if let Some(slot) = self.pending.get_mut(..tail.len()) {
            slot.copy_from_slice(tail);
        }
        self.filled = tail.len();
    }

    /// XXH64 of the input so far: `XXH64_digest`.
    pub fn digest(&self) -> u64 {
        let h = if self.total >= len64(STRIPE) { converged(&self.acc) } else { self.seed.wrapping_add(PRIME64_5) };
        finish(h.wrapping_add(self.total), self.pending.get(..self.filled).unwrap_or_default())
    }
}
