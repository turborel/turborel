//! \[EXTENSION\] A canonical Huffman code read from its code lengths
//! (RFC 1951 §3.2.2, RULES.md D193), decoded a bit at a time: the codes
//! of each length are consecutive, so a code of length n is the
//! `code - first`th symbol of that length.

use turborel::error::Result;

use super::super::malformed;
use super::super::tables::MAX_BITS;
use super::Bits;

/// A canonical code: how many codes have each length, and the symbols
/// in code order.
pub(crate) struct Huffman {
    count: [u16; MAX_BITS + 1],
    symbol: Vec<u16>,
}

impl Huffman {
    /// The code of `lengths`, one for each symbol from 0, 0 for a symbol
    /// left out. A code using more codes than its lengths allow is
    /// refused. One using fewer is taken only if it has a single code of
    /// one bit, or none, as RFC 1951 §3.2.7 allows a distance code.
    pub(crate) fn new(lengths: &[u8]) -> Result<Huffman> {
        let (code, left) = Huffman::build(lengths)?;
        let used: usize = code.count.iter().skip(1).map(|c| usize::from(*c)).sum();
        if left > 0 && used > 1 {
            return Err(malformed("an incomplete Huffman code"));
        }
        Ok(code)
    }

    /// The code of `lengths`, which must be complete unless it is empty:
    /// the code length code's rule.
    pub(crate) fn complete(lengths: &[u8]) -> Result<Huffman> {
        let (code, left) = Huffman::build(lengths)?;
        let empty = code.symbol.is_empty();
        if left > 0 && !empty {
            return Err(malformed("an incomplete code length code"));
        }
        Ok(code)
    }

    /// The code of `lengths`, and how many codes of the longest length it
    /// leaves unused.
    fn build(lengths: &[u8]) -> Result<(Huffman, i64)> {
        let mut count = [0_u16; MAX_BITS + 1];
        for length in lengths {
            let slot = count.get_mut(usize::from(*length)).ok_or_else(|| malformed("a code length past 15"))?;
            *slot = slot.saturating_add(1);
        }
        let mut left: i64 = 1;
        for c in count.iter().skip(1) {
            left = left.saturating_mul(2).saturating_sub(i64::from(*c));
            if left < 0 {
                return Err(malformed("an over-subscribed Huffman code"));
            }
        }
        let mut offsets = [0_usize; MAX_BITS + 2];
        for len in 1..=MAX_BITS {
            let before = offsets.get(len).copied().unwrap_or(0);
            let here = usize::from(count.get(len).copied().unwrap_or(0));
            if let Some(next) = offsets.get_mut(len.saturating_add(1)) {
                *next = before.saturating_add(here);
            }
        }
        let used = offsets.get(MAX_BITS.saturating_add(1)).copied().unwrap_or(0);
        let mut symbol = vec![0_u16; used];
        for (s, length) in lengths.iter().enumerate() {
            let Some(offset) = offsets.get_mut(usize::from(*length)).filter(|_| *length != 0) else { continue };
            if let Some(slot) = symbol.get_mut(*offset) {
                *slot = u16::try_from(s).unwrap_or(0);
            }
            *offset = offset.saturating_add(1);
        }
        Ok((Huffman { count, symbol }, left))
    }

    /// The next symbol of `bits`.
    pub(crate) fn decode(&self, bits: &mut Bits<'_>) -> Result<u16> {
        let (mut code, mut first, mut index) = (0_usize, 0_usize, 0_usize);
        for count in self.count.iter().skip(1) {
            code |= usize::try_from(bits.bits(1)?).unwrap_or(0);
            let count = usize::from(*count);
            if code.wrapping_sub(first) < count {
                let at = index.saturating_add(code.saturating_sub(first));
                return self.symbol.get(at).copied().ok_or_else(|| malformed("a Huffman code out of range"));
            }
            index = index.saturating_add(count);
            first = first.saturating_add(count) << 1_u32;
            code <<= 1_u32;
        }
        Err(malformed("a code its block's Huffman code does not have"))
    }
}
