//! \[DESIGN\] RULES.md D364: least significant digit radix sort of keys of
//! at most 16 bytes, each beside its row's place, by digits of 8 bits
//! (Kim et al., "Sort vs. Hash Revisited", PVLDB 2009; Polychroniou and
//! Ross 2014). Each pass is stable, so rows of equal keys keep the order
//! they came in. Every digit's counts are made in one pass first, and a
//! digit all keys share is skipped, as the digits above a key's least
//! bytes usually are. Two vectors take turns as the source and the
//! destination of a pass.

/// The values a digit takes.
const RADIX: usize = 256;

/// The bits of a digit.
const DIGIT_BITS: u32 = 8;

/// The bits of a digit, as a mask.
const DIGIT_MASK: u8 = u8::MAX;

/// A key a radix sort sorts: an unsigned integer, read a digit at a time.
pub(super) trait Key: Copy + Default {
    /// Its digits.
    const DIGITS: u32;

    /// Its digit `d`, from the least significant.
    fn digit(self, d: u32) -> usize;
}

impl Key for u64 {
    const DIGITS: u32 = u64::BITS.div_ceil(DIGIT_BITS);

    fn digit(self, d: u32) -> usize {
        usize::try_from(self.checked_shr(d.saturating_mul(DIGIT_BITS)).unwrap_or(0) & u64::from(DIGIT_MASK)).unwrap_or(0)
    }
}

impl Key for u128 {
    const DIGITS: u32 = u128::BITS.div_ceil(DIGIT_BITS);

    fn digit(self, d: u32) -> usize {
        usize::try_from(self.checked_shr(d.saturating_mul(DIGIT_BITS)).unwrap_or(0) & u128::from(DIGIT_MASK)).unwrap_or(0)
    }
}

/// `pairs`, sorted by their keys, stably.
pub(super) fn sort<K: Key>(pairs: Vec<(K, u32)>) -> Vec<(K, u32)> {
    let counts = counted(&pairs);
    let mut from = pairs;
    let mut to: Vec<(K, u32)> = vec![(K::default(), 0); from.len()];
    for (d, count) in (0_u32..).zip(&counts) {
        // A digit every key has alike orders nothing.
        if count.contains(&from.len()) {
            continue;
        }
        let mut next = [0_usize; RADIX];
        let mut sum = 0_usize;
        for (n, c) in next.iter_mut().zip(count) {
            *n = sum;
            sum = sum.saturating_add(*c);
        }
        for &pair in &from {
            if let Some(at) = next.get_mut(pair.0.digit(d)) {
                if let Some(slot) = to.get_mut(*at) {
                    *slot = pair;
                }
                *at = at.saturating_add(1);
            }
        }
        std::mem::swap(&mut from, &mut to);
    }
    from
}

/// Each digit's counts of each value, over `pairs`.
fn counted<K: Key>(pairs: &[(K, u32)]) -> Vec<[usize; RADIX]> {
    let mut counts = vec![[0_usize; RADIX]; usize::try_from(K::DIGITS).unwrap_or(0)];
    for &(key, _) in pairs {
        for (d, count) in (0_u32..).zip(counts.iter_mut()) {
            if let Some(c) = count.get_mut(key.digit(d)) {
                *c = c.saturating_add(1);
            }
        }
    }
    counts
}
