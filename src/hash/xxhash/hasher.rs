//! \[DESIGN\] XXH3-64 as a `std::hash::Hasher` (RULES.md D460): its value
//! is exactly XXH3-64 of the bytes written, by its seed, every integer
//! written little-endian, so a hash is the same on every machine and may
//! be kept. The bytes of a key wait in a buffer of 64 on the stack, so a
//! key of a few words is one short input's few multiplications; a longer
//! key streams.

use std::hash::{BuildHasher, Hasher};

use super::{Xxh3, xxh3_64_with_seed};

/// Bytes a hasher holds before it streams.
const INLINE: usize = 64;

/// \[DESIGN\] A `Hasher` whose value is XXH3-64 of the bytes written.
#[derive(Clone)]
pub struct Xxh3Hasher {
    seed: u64,
    inline: [u8; INLINE],
    len: usize,
    /// The state, once the bytes written pass `INLINE`.
    streamed: Option<Box<Xxh3<'static>>>,
}

impl Xxh3Hasher {
    /// A hasher of no bytes, by `seed`.
    pub fn with_seed(seed: u64) -> Xxh3Hasher {
        Xxh3Hasher { seed, inline: [0; INLINE], len: 0, streamed: None }
    }
}

impl Xxh3Hasher {
    /// Writes the `N` bytes of an integer: a store of known width while
    /// the buffer has room, no copy of a length found at run time.
    #[inline]
    fn fixed<const N: usize>(&mut self, bytes: [u8; N]) {
        if self.streamed.is_none()
            && let Some(slot) = self.inline.get_mut(self.len..).and_then(<[u8]>::first_chunk_mut::<N>)
        {
            *slot = bytes;
            self.len = self.len.wrapping_add(N);
            return;
        }
        self.write(&bytes);
    }
}

impl Default for Xxh3Hasher {
    fn default() -> Xxh3Hasher {
        Xxh3Hasher::with_seed(0)
    }
}

impl Hasher for Xxh3Hasher {
    fn write(&mut self, bytes: &[u8]) {
        if let Some(state) = &mut self.streamed {
            state.update(bytes);
            return;
        }
        let end = self.len.saturating_add(bytes.len());
        if let Some(slot) = self.inline.get_mut(self.len..end) {
            slot.copy_from_slice(bytes);
            self.len = end;
            return;
        }
        let mut state = Box::new(Xxh3::with_seed(self.seed));
        state.update(self.inline.get(..self.len).unwrap_or_default());
        state.update(bytes);
        self.streamed = Some(state);
    }
    fn write_u8(&mut self, n: u8) {
        self.fixed(n.to_le_bytes());
    }
    fn write_u16(&mut self, n: u16) {
        self.fixed(n.to_le_bytes());
    }
    fn write_u32(&mut self, n: u32) {
        self.fixed(n.to_le_bytes());
    }
    fn write_u64(&mut self, n: u64) {
        self.fixed(n.to_le_bytes());
    }
    fn write_u128(&mut self, n: u128) {
        self.fixed(n.to_le_bytes());
    }
    /// A `usize` as 8 bytes, so a hash is the same of every word width.
    fn write_usize(&mut self, n: usize) {
        self.fixed(u64::try_from(n).unwrap_or(u64::MAX).to_le_bytes());
    }
    fn finish(&self) -> u64 {
        match &self.streamed {
            Some(state) => state.digest64(),
            None => xxh3_64_with_seed(self.inline.get(..self.len).unwrap_or_default(), self.seed),
        }
    }
}

/// \[DESIGN\] Makes `Xxh3Hasher`s of one seed: the `BuildHasher` of the
/// engine's own hash maps (`Xxh3Map`, `Xxh3Set`).
#[derive(Debug, Clone, Copy, Default)]
pub struct Xxh3BuildHasher {
    seed: u64,
}

impl Xxh3BuildHasher {
    /// Hashers of `seed`.
    pub fn with_seed(seed: u64) -> Xxh3BuildHasher {
        Xxh3BuildHasher { seed }
    }
}

impl BuildHasher for Xxh3BuildHasher {
    type Hasher = Xxh3Hasher;
    fn build_hasher(&self) -> Xxh3Hasher {
        Xxh3Hasher::with_seed(self.seed)
    }
}

/// A `HashMap` hashing its keys by XXH3-64.
pub type Xxh3Map<K, V> = std::collections::HashMap<K, V, Xxh3BuildHasher>;
/// A `HashSet` hashing its values by XXH3-64.
pub type Xxh3Set<T> = std::collections::HashSet<T, Xxh3BuildHasher>;
