//! \[DESIGN\] XXH3-64 as a `std::hash::Hasher` (RULES.md D460): its value
//! is exactly XXH3-64 of the bytes written, by its seed, every integer
//! written little-endian, so a hash is the same on every machine and may
//! be kept. The first 16 bytes of a key wait in a head of 16 on the stack,
//! an integer stored at its width, and `finish` hashes them once by XXH3's
//! own path of 0 to 16 bytes, called straight: a key of a word or two is a
//! few multiplications. A longer key moves to a buffer of 128, made only
//! then, for the path of 17 to 128. Only a key past 128 bytes streams.

use std::hash::{BuildHasher, Hasher};

use super::Xxh3;
use super::long::{hash64_head, hash64_short_key};

/// Bytes the head holds: the most XXH3's shortest paths take.
const HEAD: usize = 16;
/// Bytes a hasher holds before it streams: the most XXH3's mid path takes.
const INLINE: usize = 128;

/// \[DESIGN\] A `Hasher` whose value is XXH3-64 of the bytes written.
#[derive(Clone)]
pub struct Xxh3Hasher {
    seed: u64,
    /// The bytes written so far.
    len: usize,
    /// The bytes while `len` is at most `HEAD`.
    head: [u8; HEAD],
    /// The bytes while `len` is past `HEAD` and at most `INLINE`: a new
    /// hasher writes only its tag.
    wide: Option<[u8; INLINE]>,
    /// The state, once the bytes written pass `INLINE`.
    streamed: Option<Box<Xxh3<'static>>>,
}

impl Xxh3Hasher {
    /// A hasher of no bytes, by `seed`.
    #[inline]
    pub fn with_seed(seed: u64) -> Xxh3Hasher {
        Xxh3Hasher { seed, len: 0, head: [0; HEAD], wide: None, streamed: None }
    }

    /// Writes the `N` bytes of an integer: a store of known width while
    /// the head has room, no copy of a length found at run time.
    #[inline]
    fn fixed<const N: usize>(&mut self, bytes: [u8; N]) {
        if let Some(slot) = self.head.get_mut(self.len..).and_then(<[u8]>::first_chunk_mut::<N>) {
            *slot = bytes;
            self.len = self.len.wrapping_add(N);
            return;
        }
        self.write(&bytes);
    }

    /// The bytes held, while they are at most `INLINE`.
    #[inline]
    fn held(&self) -> &[u8] {
        if let Some(bytes) = self.head.get(..self.len) {
            return bytes;
        }
        self.wide.as_ref().and_then(|wide| wide.get(..self.len)).unwrap_or_default()
    }

    /// Writes `bytes` past the head: into the buffer of 128, made now if
    /// it is not, or into the stream once they pass it.
    #[inline]
    fn write_wide(&mut self, bytes: &[u8], end: usize) {
        if self.streamed.is_none()
            && end <= INLINE
            && let Some(slot) = self.wide.get_or_insert_with(|| widened(self.head)).get_mut(self.len..end)
        {
            slot.copy_from_slice(bytes);
            self.len = end;
            return;
        }
        self.stream(bytes, end);
    }

    /// Writes `bytes` to the stream, begun now from the bytes held if it
    /// is not yet.
    #[inline(never)]
    fn stream(&mut self, bytes: &[u8], end: usize) {
        if self.streamed.is_none() {
            let mut state = Box::new(Xxh3::with_seed(self.seed));
            state.update(self.held());
            self.streamed = Some(state);
        }
        if let Some(state) = &mut self.streamed {
            state.update(bytes);
        }
        self.len = end;
    }
}

impl Xxh3Hasher {
    /// The hash of a key past 16 bytes: by the path of 17 to 128 bytes, or
    /// of its stream.
    #[inline(never)]
    fn finish_wide(&self) -> u64 {
        match &self.streamed {
            Some(state) => state.digest64(),
            None => hash64_short_key(self.held(), self.seed),
        }
    }
}

/// A buffer of 128 beginning with `head`.
fn widened(head: [u8; HEAD]) -> [u8; INLINE] {
    let mut wide = [0; INLINE];
    if let Some(start) = wide.first_chunk_mut::<HEAD>() {
        *start = head;
    }
    wide
}

impl Default for Xxh3Hasher {
    fn default() -> Xxh3Hasher {
        Xxh3Hasher::with_seed(0)
    }
}

impl Hasher for Xxh3Hasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let end = self.len.saturating_add(bytes.len());
        if let Some(slot) = self.head.get_mut(self.len..end) {
            slot.copy_from_slice(bytes);
            self.len = end;
            return;
        }
        self.write_wide(bytes, end);
    }
    #[inline]
    fn write_u8(&mut self, n: u8) {
        self.fixed(n.to_le_bytes());
    }
    #[inline]
    fn write_u16(&mut self, n: u16) {
        self.fixed(n.to_le_bytes());
    }
    #[inline]
    fn write_u32(&mut self, n: u32) {
        self.fixed(n.to_le_bytes());
    }
    #[inline]
    fn write_u64(&mut self, n: u64) {
        self.fixed(n.to_le_bytes());
    }
    #[inline]
    fn write_u128(&mut self, n: u128) {
        self.fixed(n.to_le_bytes());
    }
    /// A `usize` as 8 bytes, so a hash is the same of every word width.
    #[inline]
    fn write_usize(&mut self, n: usize) {
        self.fixed(u64::try_from(n).unwrap_or(u64::MAX).to_le_bytes());
    }
    /// A key of at most 16 bytes hashes here, inline, so a key of known
    /// width folds to its one path; a longer key hashes out of line.
    #[inline]
    fn finish(&self) -> u64 {
        if let Some(bytes) = self.head.get(..self.len) {
            return hash64_head(bytes, self.seed);
        }
        self.finish_wide()
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
    #[inline]
    fn build_hasher(&self) -> Xxh3Hasher {
        Xxh3Hasher::with_seed(self.seed)
    }
}

/// A `HashMap` hashing its keys by XXH3-64.
pub type Xxh3Map<K, V> = std::collections::HashMap<K, V, Xxh3BuildHasher>;
/// A `HashSet` hashing its values by XXH3-64.
pub type Xxh3Set<T> = std::collections::HashSet<T, Xxh3BuildHasher>;
