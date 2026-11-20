//! \[DESIGN\] Normal keys (RULES.md D364, D380): a vector of integers
//! written into the fixed-width keys of a sort, each as its distance above a
//! base, big-endian, so the keys' order as bytes is the integers' order.

use crate::storage::mask::index;

/// Writes into `keys`, one key of `width` bytes after another, at byte
/// `at` of each, the value of `values` at each of `rows` in turn: its
/// distance above `base` as `bytes` big-endian bytes, every one inverted
/// if `invert`. None if a value is below `base` or past `bytes`, a row past
/// the end, or a field past a key; `keys` are then partly written.
pub fn normalize_i64(
    (values, rows): (&[i64], &[u32]),
    (base, bytes, invert): (i64, usize, bool),
    (width, at): (usize, usize),
    keys: &mut [u8],
) -> Option<()> {
    let end = at.checked_add(bytes)?;
    let skip = size_of::<u64>().checked_sub(bytes)?;
    if end > width {
        return None;
    }
    let flip = if invert { u8::MAX } else { 0 };
    for (key, &r) in keys.chunks_exact_mut(width).zip(rows) {
        let value = *values.get(index(r))?;
        if value < base {
            return None;
        }
        // At or above the base, the distance fits 64 bits unsigned.
        let distance = value.wrapping_sub(base).cast_unsigned();
        let be = distance.to_be_bytes();
        let (high, low) = be.split_at_checked(skip)?;
        if high.iter().any(|&b| b != 0) {
            return None;
        }
        for (to, from) in key.get_mut(at..end)?.iter_mut().zip(low) {
            *to = from ^ flip;
        }
    }
    Some(())
}
