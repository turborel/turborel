//! \[DESIGN\] The graph's seeded pseudo-random numbers: SplitMix64 (Steele,
//! Lea and Flood, "Fast Splittable Pseudorandom Number Generators", OOPSLA
//! 2014). Vamana's random initial graph and its random permutation
//! [DiskANN, NeurIPS 2019 Algorithm 3] come from the seed kept in the meta
//! page, never from a clock, so a build is the same on every run.

/// SplitMix64's increment, the golden ratio's odd 64-bit fraction.
const GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;
/// SplitMix64's first output mix multiplier.
const MIX_1: u64 = 0xBF58_476D_1CE4_E5B9;
/// SplitMix64's second output mix multiplier.
const MIX_2: u64 = 0x94D0_49BB_1331_11EB;
/// The first shift of the output mix.
const SHIFT_1: u32 = 30;
/// The second shift of the output mix.
const SHIFT_2: u32 = 27;
/// The last shift of the output mix.
const SHIFT_3: u32 = 31;

/// A SplitMix64 generator.
pub(super) struct Random {
    state: u64,
}

impl Random {
    /// A generator from `seed`.
    pub(super) fn new(seed: u64) -> Random {
        Random { state: seed }
    }

    /// The next 64 random bits.
    pub(super) fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(GAMMA);
        let mut z = self.state;
        z = (z ^ (z >> SHIFT_1)).wrapping_mul(MIX_1);
        z = (z ^ (z >> SHIFT_2)).wrapping_mul(MIX_2);
        z ^ (z >> SHIFT_3)
    }

    /// A number below `n`, or 0 if `n` is 0. The modulo bias is below
    /// n / 2^64, far under anything a graph's quality could show.
    pub(super) fn below(&mut self, n: usize) -> usize {
        let n64 = u64::try_from(n).unwrap_or(u64::MAX);
        usize::try_from(self.next().checked_rem(n64).unwrap_or(0)).unwrap_or(0)
    }

    /// `items` in a random order (Fisher and Yates, Durstenfeld's form).
    pub(super) fn shuffle<T>(&mut self, items: &mut [T]) {
        let mut left = items.len();
        while left > 1 {
            let pick = self.below(left);
            left = left.saturating_sub(1);
            items.swap(pick, left);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] The same seed gives the same sequence and a
    /// shuffle is a permutation, so builds repeat exactly (R-TEST-5).
    #[test]
    fn seeded_sequences_repeat_and_shuffles_permute() {
        let (mut a, mut b) = (Random::new(7), Random::new(7));
        assert!((0_u32..100).all(|_| a.next() == b.next()));
        let mut items: Vec<u32> = (0..1000).collect();
        Random::new(3).shuffle(&mut items);
        assert_ne!(items, (0..1000).collect::<Vec<_>>());
        items.sort_unstable();
        assert_eq!(items, (0..1000).collect::<Vec<_>>());
        assert_eq!(Random::new(1).below(0), 0);
        assert!((0_u32..1000).all(|_| a.below(10) < 10));
    }
}
