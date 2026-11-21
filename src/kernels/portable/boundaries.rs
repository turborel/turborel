//! \[DESIGN\] Boundaries (RULES.md D560): where a run of rows in order
//! changes its key, as a window finds its partitions and peer groups of
//! the normal keys a sort writes (`exec/sort/normal.rs`), each row's key
//! compared with the one before it over a span of their bytes.

/// The bits of a word of a mask.
const WORD_BITS: usize = 64;

/// Appends to `out` the words of a mask of the `keys.len() / width` keys
/// of `width` bytes each: bit `r` set, for `r` from 1, if bytes `from` up
/// to `to` of key `r` differ from those of key `r - 1`. Bit 0 is never
/// set, nor any past the last key; of a width of 0 or a span of no bytes
/// none is.
pub fn boundaries(keys: &[u8], width: usize, (from, to): (usize, usize), out: &mut Vec<u64>) {
    let count = keys.len().checked_div(width).unwrap_or_default();
    let start = out.len();
    out.resize(start.saturating_add(count.div_ceil(WORD_BITS)), 0);
    if from >= to || to > width {
        return;
    }
    let spans = keys.chunks_exact(width).map(|key| key.get(from..to).unwrap_or_default());
    let pairs = spans.clone().zip(spans.skip(1));
    let words = out.get_mut(start..).unwrap_or_default();
    for (r, (before, after)) in (1_usize..).zip(pairs) {
        if before == after {
            continue;
        }
        let bit = u32::try_from(r.checked_rem(WORD_BITS).unwrap_or_default()).unwrap_or_default();
        if let Some(word) = words.get_mut(r.checked_div(WORD_BITS).unwrap_or_default()) {
            *word |= 1_u64.checked_shl(bit).unwrap_or_default();
        }
    }
}
