//! \[DESIGN\] A blocked Bloom filter's probe (RULES.md D369): a block of 512
//! bits, eight words, for each hash, picked by its high 28 bits, and four
//! of the block's bits by four 9-bit fields of its low 36, so a probe
//! reads one cache line, as Putze, Sanders and Singler (2007) block a
//! filter, at about the false positives of an unblocked one of the same
//! bits a key.

/// \[DESIGN\] Words of a block: 512 bits.
pub const BLOCK_WORDS: usize = 8;

/// \[DESIGN\] Bits a key sets in its block.
const PROBES: u32 = 4;

/// \[DESIGN\] Bits of a hash that pick a bit of a block of 512.
const FIELD: u32 = 9;

/// \[DESIGN\] How far a hash is turned to give the 28 bits its block is
/// picked by, above the four fields its bits are.
const BLOCK_SHIFT: u32 = 36;

/// The place of the block of hash `h` among `blocks`, `h`'s high 28 bits
/// scaled to their count.
pub fn block_of(h: u64, blocks: usize) -> usize {
    let count = u64::try_from(blocks).unwrap_or(u64::MAX);
    let picked = (h >> BLOCK_SHIFT).wrapping_mul(count) >> (u64::BITS.saturating_sub(BLOCK_SHIFT));
    usize::try_from(picked).unwrap_or(0)
}

/// The four bits of hash `h` in its block: each a word's place and a bit
/// of that word.
pub fn bits_of(h: u64) -> [(usize, u64); 4] {
    let mut out = [(0_usize, 0_u64); 4];
    for (n, slot) in (0..PROBES).zip(out.iter_mut()) {
        let field = (h >> n.saturating_mul(FIELD)) & ((1_u64 << FIELD) - 1);
        let word = usize::try_from(field >> u64::BITS.trailing_zeros()).unwrap_or(0);
        *slot = (word, 1_u64 << (field & u64::from(u64::BITS - 1)));
    }
    out
}

/// Whether the filter of `words` may hold hash `h`: every one of its bits
/// set in its block.
pub fn may_hold(words: &[u64], h: u64) -> bool {
    let block = block_of(h, words.len().checked_shr(BLOCK_WORDS.trailing_zeros()).unwrap_or(0));
    let Some(held) = words.get(block.saturating_mul(BLOCK_WORDS)..block.saturating_add(1).saturating_mul(BLOCK_WORDS)) else {
        return false;
    };
    bits_of(h).iter().all(|&(w, bit)| held.get(w).is_some_and(|&x| x & bit != 0))
}

/// Appends to `out` the place of each of `hashes` the filter of `words`
/// may hold, in order.
pub fn bloom_probe(words: &[u64], hashes: &[u64], out: &mut Vec<u32>) {
    out.extend(hashes.iter().zip(0_u32..).filter(|&(&h, _)| may_hold(words, h)).map(|(_, n)| n));
}
