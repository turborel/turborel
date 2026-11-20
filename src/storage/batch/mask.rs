//! \[DESIGN\] A bit for each row of a vector (RULES.md D354): which rows of
//! a column are null, or which truth values are true. The bits are kept
//! least significant first in 64-bit words, as the column store's chunks
//! keep their bitmaps, so a kernel reads 64 rows a word, and the bits set
//! are counted once, as they are set.

/// \[DESIGN\] Bits in a word of a mask.
pub const WORD_BITS: usize = 64;
/// The shift from a bit's place to its word's, as `WORD_BITS` is 2^6.
const WORD_SHIFT: u32 = 6;
/// The bits of a place within its word.
const IN_WORD: usize = WORD_BITS - 1;
/// The shift from a bit's place to its byte's, 2^3 bits a byte.
const BYTE_SHIFT: u32 = 3;
/// The bits of a place within its byte.
const IN_BYTE: usize = 7;
/// Bytes in a word.
const WORD_BYTES: usize = 8;

/// \[DESIGN\] A bit for each of `len` rows, least significant first in
/// 64-bit words; no bit past `len` is set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mask {
    words: Vec<u64>,
    len: usize,
    set: usize,
}

/// The word of bit `n`, and the bit in it.
fn place(n: usize) -> (usize, u64) {
    (n >> WORD_SHIFT, 1_u64 << (n & IN_WORD))
}

impl Mask {
    /// `len` bits, none set.
    pub fn none(len: usize) -> Mask {
        Mask { words: vec![0; len.div_ceil(WORD_BITS)], len, set: 0 }
    }

    /// A bit for each of `bits`, set where it is true.
    pub fn from_bools(bits: &[bool]) -> Mask {
        let mut mask = Mask::none(bits.len());
        for (word, chunk) in mask.words.iter_mut().zip(bits.chunks(WORD_BITS)) {
            *word = chunk.iter().rev().fold(0_u64, |w, &b| (w << 1_u32) | u64::from(b));
        }
        mask.set = mask.recount();
        mask
    }

    /// `len` bits of `bytes`, least significant first, from bit `from`, as
    /// a chunk's bitmap keeps them; a bit past the bytes unset.
    pub fn from_lsb_bytes(bytes: &[u8], from: usize, len: usize) -> Mask {
        let mut mask = Mask::none(len);
        let bit = |n: usize| bytes.get(n >> BYTE_SHIFT).is_some_and(|b| (b >> (n & IN_BYTE)) & 1 == 1);
        let shift = from & IN_BYTE;
        for (w, word) in mask.words.iter_mut().enumerate() {
            let first = from.saturating_add(w.saturating_mul(WORD_BITS));
            if shift == 0 {
                let at = first >> BYTE_SHIFT;
                let mut eight = [0_u8; WORD_BYTES];
                for (slot, b) in eight.iter_mut().zip(bytes.iter().skip(at)) {
                    *slot = *b;
                }
                *word = u64::from_le_bytes(eight);
            } else {
                *word = (0..WORD_BITS).rev().fold(0_u64, |acc, n| (acc << 1_u32) | u64::from(bit(first.saturating_add(n))));
            }
        }
        mask.trim();
        mask.set = mask.recount();
        mask
    }

    /// The bits set among its words, each counted.
    fn recount(&self) -> usize {
        self.words.iter().map(|w| usize::try_from(w.count_ones()).unwrap_or(0)).sum()
    }

    /// Clears the bits of its last word past `len`.
    fn trim(&mut self) {
        let tail = self.len & IN_WORD;
        if tail > 0
            && let Some(last) = self.words.last_mut()
        {
            *last &= (1_u64 << tail).wrapping_sub(1);
        }
    }

    /// Whether bit `n` is set: never past `len`.
    pub fn get(&self, n: usize) -> bool {
        let (w, bit) = place(n);
        self.words.get(w).is_some_and(|word| word & bit != 0)
    }

    /// Sets bit `n`; none past `len`.
    pub fn set(&mut self, n: usize) {
        let (w, bit) = place(n);
        if n < self.len
            && let Some(word) = self.words.get_mut(w)
            && *word & bit == 0
        {
            *word |= bit;
            self.set = self.set.saturating_add(1);
        }
    }

    /// Adds a bit past the last, set if `bit`.
    pub fn push(&mut self, bit: bool) {
        if self.len & IN_WORD == 0 {
            self.words.push(0);
        }
        self.len = self.len.saturating_add(1);
        if bit {
            self.set(self.len.saturating_sub(1));
        }
    }

    /// How many bits are set.
    pub fn count(&self) -> usize {
        self.set
    }

    /// How many bits it has.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether it has no bits.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Its words, least significant bit first.
    pub fn words(&self) -> &[u64] {
        &self.words
    }

    /// Whether any bit is set.
    pub fn any(&self) -> bool {
        self.set > 0
    }

    /// Whether any of the bits at `rows` is set, or any at all.
    pub fn any_of(&self, rows: Option<&[u32]>) -> bool {
        match rows {
            Some(rows) => self.any() && rows.iter().any(|&r| self.get(index(r))),
            None => self.any(),
        }
    }

    /// The bits at `picks`, in order; none if no bit of them is set.
    pub fn gathered(&self, picks: &[u32]) -> Option<Mask> {
        if !self.any() {
            return None;
        }
        let mut out = Mask::none(picks.len());
        for (n, &p) in picks.iter().enumerate() {
            if self.get(index(p)) {
                out.set(n);
            }
        }
        out.any().then_some(out)
    }

    /// The bits of `self` or `other`, of the same length.
    #[must_use]
    pub fn or(&self, other: &Mask) -> Mask {
        let words = self.words.iter().zip(&other.words).map(|(a, b)| a | b).collect();
        self.with_words(words)
    }

    /// The bits of `self` and `other`, of the same length.
    #[must_use]
    pub fn and(&self, other: &Mask) -> Mask {
        let words = self.words.iter().zip(&other.words).map(|(a, b)| a & b).collect();
        self.with_words(words)
    }

    /// The bits of `self` not set in `other`, of the same length.
    #[must_use]
    pub fn and_not(&self, other: &Mask) -> Mask {
        let words = self.words.iter().zip(other.words.iter().chain(std::iter::repeat(&0))).map(|(a, b)| a & !b).collect();
        self.with_words(words)
    }

    /// A mask of its length with `words`.
    fn with_words(&self, words: Vec<u64>) -> Mask {
        let mut mask = Mask { words, len: self.len, set: 0 };
        mask.words.resize(self.len.div_ceil(WORD_BITS), 0);
        mask.trim();
        mask.set = mask.recount();
        mask
    }

    /// Its bits as bytes, least significant first, as `from_lsb_bytes`
    /// reads them: `lsb_len(len)` of them.
    pub fn lsb_bytes(&self) -> Vec<u8> {
        let mut out: Vec<u8> = self.words.iter().flat_map(|w| w.to_le_bytes()).collect();
        out.truncate(lsb_len(self.len));
        out
    }

    /// Its bits, a `bool` each.
    pub fn to_bools(&self) -> Vec<bool> {
        (0..self.len).map(|n| self.get(n)).collect()
    }
}

impl From<Vec<bool>> for Mask {
    fn from(bits: Vec<bool>) -> Mask {
        Mask::from_bools(&bits)
    }
}

/// A row's place in a vector as a `usize`: every place of a batch, and of
/// a held build side, fits.
pub fn index(r: u32) -> usize {
    usize::try_from(r).unwrap_or(usize::MAX)
}

/// The bytes `len` bits take, least significant first.
pub fn lsb_len(len: usize) -> usize {
    len.div_ceil(WORD_BYTES)
}

/// Of nulls marked one a row, a mask, none if none is null.
pub fn mask_of(nulls: &[bool]) -> Option<Mask> {
    nulls.contains(&true).then(|| Mask::from_bools(nulls))
}

#[cfg(test)]
mod tests;
