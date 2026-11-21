//! \[DESIGN\] Hashes (RULES.md D356, D573): each integer of a vector hashed
//! as a grouping's table hashes a word, XXH3-64 of its 8 bytes by the
//! seed (`hash::xxhash::Words`), its two multiplies a loop with no branch
//! the compiler vectorizes; a key's columns joined as XXH3-64 of the 16
//! bytes of two hashes; each text of a vector hashed by XXH3-64 of its
//! bytes less its trailing spaces.

use crate::hash::xxhash::{Seeded, Words};
use crate::storage::mask::index;

/// Appends to `out` the hash of each of `values` at `sel`, or of all, by
/// `seed`; that of 0 for a row past the end.
#[inline]
pub fn hash_i64(values: &[i64], seed: u64, sel: Option<&[u32]>, out: &mut Vec<u64>) {
    let words = Words::new(seed);
    let hashed = |v: i64| words.hash(v.cast_unsigned());
    match sel {
        Some(sel) => out.extend(sel.iter().map(|&r| hashed(values.get(index(r)).copied().unwrap_or(0)))),
        None => out.extend(values.iter().map(|&v| hashed(v))),
    }
}

/// Joins to each of `hashes` the hash of `next` at its place, by `seed`.
#[inline]
pub fn combine_i64(hashes: &mut [u64], next: &[u64], seed: u64) {
    let words = Words::new(seed);
    hashes.iter_mut().zip(next).for_each(|(h, &n)| *h = words.pair(*h, n));
}

/// The bytes of the `n`th text of `text` ending at `ends`, less its
/// trailing spaces; none past the end.
#[inline]
pub fn text_at<'t>((text, ends): (&'t [u8], &[usize]), n: usize) -> &'t [u8] {
    let start = n.checked_sub(1).and_then(|p| ends.get(p)).copied().unwrap_or(0);
    let bytes = ends.get(n).and_then(|&end| text.get(start..end)).unwrap_or_default();
    let kept = bytes.iter().rposition(|&b| b != b' ').map_or(0, |last| last.saturating_add(1));
    bytes.get(..kept).unwrap_or_default()
}

/// Appends to `out` the hash of each text at `sel`, or of all, of `text`
/// ending at `ends`, by `seeded`.
#[inline]
pub fn hash_texts((text, ends): (&[u8], &[usize]), seeded: &Seeded, sel: Option<&[u32]>, out: &mut Vec<u64>) {
    match sel {
        Some(sel) => out.extend(sel.iter().map(|&r| seeded.bytes(text_at((text, ends), index(r))))),
        None => out.extend((0..ends.len()).map(|n| seeded.bytes(text_at((text, ends), n)))),
    }
}
