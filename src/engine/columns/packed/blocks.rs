//! \[DESIGN\] Bit-unpacking a block at a time (RULES.md D352), after Lemire
//! and Boytsov, "Decoding Billions of Integers per Second Through
//! Vectorization", SPE 2015: the values of each width from 0 to 64 bits
//! read 64 at a time by a routine made for that width alone, its shifts
//! and masks constants the compiler unrolls, and each value added to a
//! frame's least as it is read (frame of reference fused with unpacking).
//! A block of 64 values of `w` bits is exactly `8 * w` bytes, read as `w`
//! little-endian words. The format is the one `packed.rs` writes, values
//! one after another from the least significant bit; the values before
//! the first whole block and after the last are read one at a time.

use super::{mask, packed_len, unpack};

/// The values of a block.
pub(super) const BLOCK: usize = 64;
/// The bytes of a word a block is read in.
const WORD_BYTES: usize = 8;
/// The bits of a word, as a shift's amount.
const WORD_BITS: usize = 64;

/// A routine unpacking one block of a width: the block's bytes, the least
/// added to each value, and where the 64 values go.
type Unpack = fn(&[u8], i64, &mut [i64; BLOCK]) -> Option<()>;

/// Of each width, its routine: `block::<W>` made once a width, so `W` is a
/// constant in each.
macro_rules! routines {
    ($($w:literal)*) => {
        [$(block::<$w>),*]
    };
}

/// The routine of each width from 0 to 64 bits, by width.
const UNPACK: [Unpack; 65] = routines!(
    0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32
    33 34 35 36 37 38 39 40 41 42 43 44 45 46 47 48 49 50 51 52 53 54 55 56 57 58 59 60 61 62 63 64
);

/// The 64 values of `W` bits `bytes` begin with, each plus `least`,
/// wrapping, into `out`: none if `bytes` hold fewer than `8 * W`. A value
/// whose bits run past its word takes the rest from the next one. Adding
/// wrapping gives back the integer of 64 bits a frame was made of, its
/// difference from the least below 2^64 (`encoded.rs`).
fn block<const W: usize>(bytes: &[u8], least: i64, out: &mut [i64; BLOCK]) -> Option<()> {
    let mut words = [0_u64; W];
    let given = bytes.get(..W.checked_mul(WORD_BYTES)?)?;
    for (word, b) in words.iter_mut().zip(given.as_chunks::<WORD_BYTES>().0) {
        *word = u64::from_le_bytes(*b);
    }
    let low_bits = mask(u32::try_from(W).ok()?);
    for (n, slot) in out.iter_mut().enumerate() {
        let bit = n.wrapping_mul(W);
        let (at, shift) = (bit.checked_div(WORD_BITS)?, u32::try_from(bit.checked_rem(WORD_BITS)?).ok()?);
        let low = words.get(at).copied().unwrap_or(0).wrapping_shr(shift);
        // Only when the value runs past its word is `shift` above 0, so
        // the left shift is below 64.
        let high = if usize::try_from(shift).ok()?.wrapping_add(W) > WORD_BITS {
            words.get(at.wrapping_add(1)).copied().unwrap_or(0).wrapping_shl(u64::BITS.wrapping_sub(shift))
        } else {
            0
        };
        *slot = least.wrapping_add(i64::from_le_bytes(((low | high) & low_bits).to_le_bytes()));
    }
    Some(())
}

/// The values from the `from`th to the `to`th of `bits` bits packed in
/// `bytes`, each plus `least`, wrapping, pushed to `out`: whole blocks by
/// their width's routine, the others one at a time. None if `bytes` hold
/// fewer values, or `bits` is above 64.
pub(in crate::engine::columns) fn unpack_words(
    bytes: &[u8],
    (bits, least): (u32, i64),
    (from, to): (usize, usize),
    out: &mut Vec<i64>,
) -> Option<()> {
    if from > to || bytes.len() < packed_len(to, bits)? {
        return None;
    }
    let width = usize::try_from(bits).ok()?;
    let routine = UNPACK.get(width)?;
    let plus = |d: u64| least.wrapping_add(i64::from_le_bytes(d.to_le_bytes()));
    let first_block = from.div_ceil(BLOCK);
    let head = first_block.checked_mul(BLOCK)?.min(to);
    out.reserve(to.saturating_sub(from));
    unpack(bytes, bits, (from, head), |d| out.push(plus(d)))?;
    // The blocks whose bytes are all there: the last may end in a byte
    // past the values', which a chunk need not hold.
    let block_bytes = width.checked_mul(WORD_BYTES)?;
    let whole = to.checked_div(BLOCK)?.min(bytes.len().checked_div(block_bytes).unwrap_or(usize::MAX));
    let blocks = whole.saturating_sub(first_block);
    let start = out.len();
    out.resize(start.checked_add(blocks.checked_mul(BLOCK)?)?, 0);
    let slots = out.get_mut(start..)?.as_chunks_mut::<BLOCK>().0;
    for (b, slot) in (first_block..).zip(slots.iter_mut()) {
        let at = b.checked_mul(block_bytes)?;
        routine(bytes.get(at..)?, least, slot)?;
    }
    let tail = head.checked_add(blocks.checked_mul(BLOCK)?)?;
    unpack(bytes, bits, (tail, to), |d| out.push(plus(d)))
}

#[cfg(test)]
mod tests;
