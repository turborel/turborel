//! The simulator's one source of chance: SplitMix64 (G. Steele, D. Lea,
//! F. Flood, "Fast Splittable Pseudorandom Number Generators", OOPSLA
//! 2014), from a seed, so a run is the same each time its seed is given.

/// A pseudorandom generator from a seed.
#[derive(Debug, Clone)]
pub struct Rng(u64);

/// SplitMix64's increment and multipliers.
const GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;
const MIX1: u64 = 0xBF58_476D_1CE4_E5B9;
const MIX2: u64 = 0x94D0_49BB_1331_11EB;
/// Per mille: chances are counted in thousandths.
pub const PER_MILLE: u32 = 1000;

impl Rng {
    /// The generator of `seed`.
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    /// The next 64 bits.
    pub fn bits(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(GAMMA);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(MIX1);
        z = (z ^ (z >> 27)).wrapping_mul(MIX2);
        z ^ (z >> 31)
    }

    /// A number below `bound`, which is not 0.
    pub fn below(&mut self, bound: u64) -> u64 {
        self.bits() % bound
    }

    /// A number from `low` through `high`.
    pub fn between(&mut self, low: u64, high: u64) -> u64 {
        low + self.below(high - low + 1)
    }

    /// True `per_mille` times in a thousand.
    pub fn chance(&mut self, per_mille: u32) -> bool {
        self.below(u64::from(PER_MILLE)) < u64::from(per_mille)
    }

    /// An index into a collection of `len` items, which is not empty.
    pub fn index(&mut self, len: usize) -> usize {
        usize::try_from(self.below(u64::try_from(len).unwrap())).unwrap()
    }
}
