//! \[DESIGN\] Sums and extremes (RULES.md D354). An exact sum of `i64`s is
//! made in two halves, each value's high 32 bits, signed, and low 32 bits,
//! unsigned, summed apart in 64-bit lanes the compiler vectorizes, a
//! chunk of values at a time small enough that neither lane overflows,
//! then joined in 128 bits: no 128-bit add a value.

use crate::kernels::Given;
use crate::storage::mask::index;

/// \[DESIGN\] Values summed in 64-bit lanes before they are joined: below
/// 2^31, so 2^31 lows of under 2^32 each, and highs of magnitude at most
/// 2^31 each, fit in 64 bits.
const CHUNK: usize = 1 << 30;

/// Bits in a half of a value.
const HALF_BITS: u32 = 32;

/// The low half of `v`, unsigned.
fn low(v: i64) -> u64 {
    u64::from_le_bytes(v.to_le_bytes()) & u64::from(u32::MAX)
}

/// The high half of `v`, signed.
fn high(v: i64) -> i64 {
    v >> HALF_BITS
}

/// The halves of `values` summed, joined in 128 bits.
fn halves(values: impl Iterator<Item = i64> + Clone) -> i128 {
    let (mut lows, mut highs) = (0_u64, 0_i64);
    for v in values {
        lows = lows.wrapping_add(low(v));
        highs = highs.wrapping_add(high(v));
    }
    (i128::from(highs) << HALF_BITS).wrapping_add(i128::from(lows))
}

/// The exact sum of `values` at `sel`, or of all.
pub fn sum_i64(values: &[i64], sel: Option<&[u32]>) -> i128 {
    match sel {
        None => values.chunks(CHUNK).map(|chunk| halves(chunk.iter().copied())).fold(0_i128, i128::wrapping_add),
        Some(sel) => sel
            .chunks(CHUNK)
            .map(|chunk| halves(chunk.iter().map(|&r| values.get(index(r)).copied().unwrap_or(0))))
            .fold(0_i128, i128::wrapping_add),
    }
}

/// The least and greatest of `values` of `given`, not null; none of no
/// row.
pub fn min_max_i64(values: &[i64], (nulls, sel): Given<'_>) -> Option<(i64, i64)> {
    let held = |n: usize| !nulls.is_some_and(|m| m.get(n));
    let fold = |best: Option<(i64, i64)>, v: i64| Some(best.map_or((v, v), |(lo, hi)| (lo.min(v), hi.max(v))));
    match sel {
        Some(sel) => sel.iter().map(|&r| index(r)).filter(|&n| held(n)).filter_map(|n| values.get(n).copied()).fold(None, fold),
        None if nulls.is_none() => values.iter().copied().fold(None, fold),
        None => values.iter().enumerate().filter(|&(n, _)| held(n)).map(|(_, &v)| v).fold(None, fold),
    }
}
