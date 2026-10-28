//! \[DESIGN\] MurmurHash3 (RULES.md D30): Austin Appleby's non-cryptographic
//! hash, placed by its author in the public domain (SMHasher,
//! `src/MurmurHash3.cpp`). Fast and well mixed, for hash tables, Bloom
//! filters and partitioning, where keys need spreading but no adversary is
//! assumed (a key chosen to collide is possible: maps keyed by data from
//! clients keep std's SipHash).
//!
//! * `Murmur3` is the x64 128-bit variant over bytes, as a `HashFunction`
//!   (`MURMUR3_X64_128`, 16 bytes, its two halves little-endian), and as
//!   `murmur3_x64_128`, giving the two halves: a Bloom filter derives its
//!   probes from them.
//! * `MurmurBuildHasher` makes `MurmurHasher`s for `std::collections`
//!   maps: integers written whole are mixed as 64-bit blocks by the same
//!   steps, with no loop over bytes, and the result goes through MurmurHash3's
//!   finalizer `fmix64`, so a key of a few integers, like a buffer pool's
//!   page, costs a few multiplications. Its values live only in memory:
//!   they are not MurmurHash3's of the same bytes, and may change.

use std::hash::{BuildHasher, Hasher};

use super::{Digest, HashFunction, HashId};

/// The id of `Murmur3` in file formats.
pub const MURMUR3_X64_128: HashId = HashId(10);

/// MurmurHash3's multipliers of the first and second 64-bit lane.
const C1: u64 = 0x87c3_7b91_1142_53d5;
const C2: u64 = 0x4cf5_ad43_2745_937f;
/// The additions after each block of the first and second lane.
const N1: u64 = 0x52dc_e729;
const N2: u64 = 0x3849_5ab5;
/// Bytes in a block of the 128-bit variant: two words, one per lane.
const BLOCK: usize = 16;
/// Bytes in a word.
const WORD: usize = 8;

/// MurmurHash3's 64-bit finalizer: every input bit affects every output
/// bit, and distinct inputs give distinct outputs.
pub fn fmix64(mut k: u64) -> u64 {
    k ^= k >> 33_u32;
    k = k.wrapping_mul(0xff51_afd7_ed55_8ccd);
    k ^= k >> 33_u32;
    k = k.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    k ^= k >> 33_u32;
    k
}

/// A word of the first lane, mixed before it joins the state.
fn mix1(k: u64) -> u64 {
    k.wrapping_mul(C1).rotate_left(31).wrapping_mul(C2)
}

/// A word of the second lane, mixed before it joins the state.
fn mix2(k: u64) -> u64 {
    k.wrapping_mul(C2).rotate_left(33).wrapping_mul(C1)
}

/// The little-endian word of up to 8 `bytes`, the missing high bytes zero.
fn word(bytes: &[u8]) -> u64 {
    let mut le = [0_u8; 8];
    if let Some(prefix) = le.get_mut(..bytes.len()) {
        prefix.copy_from_slice(bytes);
    }
    u64::from_le_bytes(le)
}

/// The state of the 128-bit variant, fed bytes in pieces.
struct State {
    h1: u64,
    h2: u64,
    /// Bytes of an unfinished block.
    pending: [u8; BLOCK],
    filled: usize,
    len: u64,
}

impl State {
    fn new(seed: u32) -> State {
        State { h1: u64::from(seed), h2: u64::from(seed), pending: [0; BLOCK], filled: 0, len: 0 }
    }

    /// Mixes in one whole block.
    fn block(&mut self, block: &[u8]) {
        let (low, high) = block.split_at(WORD);
        self.h1 ^= mix1(word(low));
        self.h1 = self.h1.rotate_left(27).wrapping_add(self.h2).wrapping_mul(5).wrapping_add(N1);
        self.h2 ^= mix2(word(high));
        self.h2 = self.h2.rotate_left(31).wrapping_add(self.h1).wrapping_mul(5).wrapping_add(N2);
    }

    fn update(&mut self, mut data: &[u8]) {
        self.len = self.len.wrapping_add(u64::try_from(data.len()).unwrap_or(u64::MAX));
        if self.filled > 0 {
            let wanted = BLOCK.saturating_sub(self.filled).min(data.len());
            let (head, rest) = data.split_at(wanted);
            if let Some(slot) = self.pending.get_mut(self.filled..self.filled.saturating_add(wanted)) {
                slot.copy_from_slice(head);
            }
            self.filled = self.filled.saturating_add(wanted);
            data = rest;
            if self.filled < BLOCK {
                return;
            }
            let pending = self.pending;
            self.block(&pending);
            self.filled = 0;
        }
        let (blocks, tail) = data.as_chunks::<BLOCK>();
        for block in blocks {
            self.block(block);
        }
        if let Some(slot) = self.pending.get_mut(..tail.len()) {
            slot.copy_from_slice(tail);
        }
        self.filled = tail.len();
    }

    fn finish(mut self) -> (u64, u64) {
        let tail = self.pending.get(..self.filled).unwrap_or_default();
        let (low, high) = tail.split_at(tail.len().min(WORD));
        if !high.is_empty() {
            self.h2 ^= mix2(word(high));
        }
        if !low.is_empty() {
            self.h1 ^= mix1(word(low));
        }
        let (mut h1, mut h2) = (self.h1 ^ self.len, self.h2 ^ self.len);
        h1 = h1.wrapping_add(h2);
        h2 = h2.wrapping_add(h1);
        h1 = fmix64(h1);
        h2 = fmix64(h2);
        h1 = h1.wrapping_add(h2);
        h2 = h2.wrapping_add(h1);
        (h1, h2)
    }
}

/// MurmurHash3 x64 128-bit of `data` with `seed`: its two 64-bit halves.
pub fn murmur3_x64_128(data: &[u8], seed: u32) -> (u64, u64) {
    let mut state = State::new(seed);
    state.update(data);
    state.finish()
}

/// \[DESIGN\] MurmurHash3 x64 128-bit, seed 0, as a `HashFunction`.
pub struct Murmur3;

impl HashFunction for Murmur3 {
    fn id(&self) -> HashId {
        MURMUR3_X64_128
    }
    fn name(&self) -> &'static str {
        "MurmurHash3_x64_128"
    }
    fn digest_len(&self) -> usize {
        BLOCK
    }
    fn hash_parts(&self, parts: &[&[u8]]) -> Digest {
        let mut state = State::new(0);
        for part in parts {
            state.update(part);
        }
        let (h1, h2) = state.finish();
        let mut bytes = [0_u8; BLOCK];
        let (low, high) = bytes.split_at_mut(WORD);
        low.copy_from_slice(&h1.to_le_bytes());
        high.copy_from_slice(&h2.to_le_bytes());
        Digest::new(&bytes).unwrap_or_default()
    }
}

/// \[DESIGN\] A `Hasher` for in-memory hash tables, after MurmurHash3: each
/// word written is mixed into the state as a block of the first lane, and
/// `finish` gives `fmix64` of the state and the bytes written.
#[derive(Clone, Copy)]
pub struct MurmurHasher {
    h: u64,
    len: u64,
}

impl MurmurHasher {
    /// Mixes in `k`, `bytes` long.
    fn mix(&mut self, k: u64, bytes: u64) {
        self.h ^= mix1(k);
        self.h = self.h.rotate_left(27).wrapping_mul(5).wrapping_add(N1);
        self.len = self.len.wrapping_add(bytes);
    }
}

impl Hasher for MurmurHasher {
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            self.mix(word(chunk), u64::try_from(chunk.len()).unwrap_or(8));
        }
    }
    fn write_u8(&mut self, n: u8) {
        self.mix(u64::from(n), 1);
    }
    fn write_u16(&mut self, n: u16) {
        self.mix(u64::from(n), 2);
    }
    fn write_u32(&mut self, n: u32) {
        self.mix(u64::from(n), 4);
    }
    fn write_u64(&mut self, n: u64) {
        self.mix(n, 8);
    }
    fn write_u128(&mut self, n: u128) {
        let [low, high] = [n, n >> 64_u32].map(|w| u64::try_from(w & u128::from(u64::MAX)).unwrap_or_default());
        self.mix(low, 8);
        self.mix(high, 8);
    }
    fn write_usize(&mut self, n: usize) {
        self.mix(u64::try_from(n).unwrap_or(u64::MAX), 8);
    }
    fn finish(&self) -> u64 {
        fmix64(self.h ^ self.len)
    }
}

/// \[DESIGN\] Makes `MurmurHasher`s from a seed: the `BuildHasher` of the
/// engine's own hash tables (`MurmurMap`).
#[derive(Clone, Copy, Default)]
pub struct MurmurBuildHasher {
    seed: u64,
}

impl MurmurBuildHasher {
    /// Hashers starting from `seed`.
    pub fn with_seed(seed: u64) -> MurmurBuildHasher {
        MurmurBuildHasher { seed }
    }
}

impl BuildHasher for MurmurBuildHasher {
    type Hasher = MurmurHasher;
    fn build_hasher(&self) -> MurmurHasher {
        MurmurHasher { h: self.seed, len: 0 }
    }
}

/// A `HashMap` hashing its keys with `MurmurHasher`.
pub type MurmurMap<K, V> = std::collections::HashMap<K, V, MurmurBuildHasher>;
/// A `HashSet` hashing its values with `MurmurHasher`.
pub type MurmurSet<T> = std::collections::HashSet<T, MurmurBuildHasher>;

// Unit tests: the reference implementation's verification value, pieces,
// and the hasher's spread (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
