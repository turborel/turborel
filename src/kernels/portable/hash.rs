//! \[DESIGN\] Hashes (RULES.md D356): each integer of a vector hashed as a
//! grouping's table hashes a word, MurmurHash3's finalizer of it after the
//! seed is added (`hash::murmur::fmix64`), a multiply-shift chain with no
//! branch the compiler vectorizes.

use crate::hash::murmur::fmix64;
use crate::storage::mask::index;

/// Appends to `out` the hash of each of `values` at `sel`, or of all, by
/// `seed`; that of 0 for a row past the end.
#[inline]
pub fn hash_i64(values: &[i64], seed: u64, sel: Option<&[u32]>, out: &mut Vec<u64>) {
    let hashed = |v: i64| fmix64(seed ^ v.cast_unsigned());
    match sel {
        Some(sel) => out.extend(sel.iter().map(|&r| hashed(values.get(index(r)).copied().unwrap_or(0)))),
        None => out.extend(values.iter().map(|&v| hashed(v))),
    }
}
