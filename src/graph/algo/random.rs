//! \[DESIGN\] The random choices of the algorithms that make them, which
//! vertices a sampled betweenness starts from and the order Louvain
//! visits vertices in: a SplitMix64 sequence (Steele, Lea and Flood,
//! "Fast Splittable Pseudorandom Number Generators", OOPSLA 2014) of the
//! caller's seed, so a seed gives the same choices on every run and
//! platform.

use super::csr::{Vertex, vertex};

/// SplitMix64's increment, the golden ratio's fraction in 64 bits.
const GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;
/// SplitMix64's first mixing multiplier.
const MIX_1: u64 = 0xBF58_476D_1CE4_E5B9;
/// SplitMix64's second mixing multiplier.
const MIX_2: u64 = 0x94D0_49BB_1331_11EB;
/// SplitMix64's first shift.
const SHIFT_1: u32 = 30;
/// SplitMix64's second shift.
const SHIFT_2: u32 = 27;
/// SplitMix64's last shift.
const SHIFT_3: u32 = 31;

/// A sequence of pseudo-random numbers of a seed.
#[derive(Debug, Clone)]
pub(crate) struct Random {
    state: u64,
}

impl Random {
    /// The sequence of `seed`.
    pub(crate) fn new(seed: u64) -> Random {
        Random { state: seed }
    }

    /// Its next number.
    pub(crate) fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(GAMMA);
        let mut z = self.state;
        z = (z ^ (z >> SHIFT_1)).wrapping_mul(MIX_1);
        z = (z ^ (z >> SHIFT_2)).wrapping_mul(MIX_2);
        z ^ (z >> SHIFT_3)
    }

    /// A number below `n`, 0 if `n` is 0.
    pub(crate) fn below(&mut self, n: usize) -> usize {
        let n64 = u64::try_from(n).unwrap_or(u64::MAX);
        usize::try_from(self.next().checked_rem(n64).unwrap_or(0)).unwrap_or(0)
    }

    /// The vertices `0..n` in an order of its choosing (Fisher and Yates,
    /// as Durstenfeld gives it).
    pub(crate) fn shuffled(&mut self, n: usize) -> Vec<Vertex> {
        let mut order: Vec<Vertex> = (0..n).map(vertex).collect();
        for i in (1..n).rev() {
            let j = self.below(i.saturating_add(1));
            order.swap(i, j);
        }
        order
    }
}
