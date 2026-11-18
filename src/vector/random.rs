//! \[DESIGN\] A deterministic pseudo-random sequence for training
//! quantizers: SplitMix64 (Steele, Lea and Flood, "Fast splittable
//! pseudorandom number generators", OOPSLA 2014). No clock and no
//! operating-system randomness, so a seed gives the same codebook on
//! every machine.

/// SplitMix64's increment, the golden ratio's 64-bit fraction.
const GAMMA: u64 = 0x9e37_79b9_7f4a_7c15;
/// SplitMix64's first mixing multiplier.
const MIX_1: u64 = 0xbf58_476d_1ce4_e5b9;
/// SplitMix64's second mixing multiplier.
const MIX_2: u64 = 0x94d0_49bb_1331_11eb;
/// SplitMix64's shifts.
const SHIFT_1: u32 = 30;
const SHIFT_2: u32 = 27;
const SHIFT_3: u32 = 31;

/// A SplitMix64 sequence.
#[derive(Debug, Clone)]
pub(crate) struct Random {
    state: u64,
}

impl Random {
    /// The sequence of `seed`.
    pub(crate) fn new(seed: u64) -> Random {
        Random { state: seed }
    }

    /// The next number.
    pub(crate) fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(GAMMA);
        let mut z = self.state;
        z = (z ^ (z >> SHIFT_1)).wrapping_mul(MIX_1);
        z = (z ^ (z >> SHIFT_2)).wrapping_mul(MIX_2);
        z ^ (z >> SHIFT_3)
    }

    /// A number below `n`, 0 if `n` is 0. The modulo bias is below
    /// `n / 2^64`, nothing for a training sample.
    pub(crate) fn below(&mut self, n: usize) -> usize {
        let n64 = u64::try_from(n).unwrap_or(u64::MAX);
        usize::try_from(self.next().checked_rem(n64).unwrap_or(0)).unwrap_or(0)
    }

    /// `items` shuffled in place (Fisher and Yates, Durstenfeld's form).
    pub(crate) fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.below(i.saturating_add(1));
            items.swap(i, j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] SplitMix64 gives the published first outputs of
    /// seed 0 (Vigna's reference implementation), so training is the same
    /// everywhere; `below` stays below its bound.
    #[test]
    fn splitmix_matches_reference() {
        let mut r = Random::new(0);
        assert_eq!(r.next(), 0xe220_a839_7b1d_cdaf);
        assert_eq!(r.next(), 0x6e78_9e6a_a1b9_65f4);
        let mut bounded = Random::new(7);
        assert!((0_i32..1_000_i32).all(|_| bounded.below(13) < 13));
        assert_eq!(bounded.below(0), 0);
        let mut items: Vec<u32> = (0..50).collect();
        Random::new(1).shuffle(&mut items);
        let mut sorted = items.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..50).collect::<Vec<_>>());
        assert_ne!(items, sorted);
    }
}
