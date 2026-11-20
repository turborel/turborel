//! \[DESIGN\] Seeded randomness and hashing for the vector index engines.
//! Every random choice derives from a seed kept in the index's metadata,
//! never the clock, so a build is reproducible and tests are
//! deterministic (R-TEST-5). The generator is SplitMix64 (Steele, Lea and
//! Flood, "Fast Splittable Pseudorandom Number Generators", OOPSLA 2014).

/// SplitMix64's increment, the golden ratio's 64 bits.
const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;
/// SplitMix64's first multiplier.
const MIX_1: u64 = 0xBF58_476D_1CE4_E5B9;
/// SplitMix64's second multiplier.
const MIX_2: u64 = 0x94D0_49BB_1331_11EB;
/// SplitMix64's shifts.
const SHIFT_1: u32 = 30;
/// The second shift.
const SHIFT_2: u32 = 27;
/// The third shift.
const SHIFT_3: u32 = 31;
/// The bits of a random word kept for a unit value: a `u32`'s.
const UNIT_SHIFT: u32 = 32;
/// 2^32, the count of unit values.
const UNIT_SPAN: f64 = 4_294_967_296.0;
/// \[DESIGN\] The highest HNSW layer: a node reaches layer l with
/// probability M^-l, so past 16 layers with M at least 2 holds fewer than
/// one node in 65,536 and stops nothing.
pub const MAX_LEVEL: u8 = 16;
/// The least M whose layers thin out: at 1 every node would reach every layer.
const MIN_M: usize = 2;

/// A SplitMix64 generator.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    /// A generator from `seed`.
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    /// The next word.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(GOLDEN);
        mixed(self.0)
    }

    /// A value in (0, 1], never 0, so its logarithm is finite.
    pub fn unit(&mut self) -> f64 {
        let high = u32::try_from(self.next_u64() >> UNIT_SHIFT).unwrap_or(u32::MAX);
        (f64::from(high) + 1.0) / UNIT_SPAN
    }

    /// A whole number below `n`, or 0 if `n` is 0.
    pub fn below(&mut self, n: usize) -> usize {
        let n = u64::try_from(n).unwrap_or(u64::MAX);
        usize::try_from(self.next_u64().checked_rem(n).unwrap_or(0)).unwrap_or(0)
    }
}

/// SplitMix64's finalizer of `z`: every bit of the input moves every bit.
pub fn mixed(mut z: u64) -> u64 {
    z = (z ^ (z >> SHIFT_1)).wrapping_mul(MIX_1);
    z = (z ^ (z >> SHIFT_2)).wrapping_mul(MIX_2);
    z ^ (z >> SHIFT_3)
}

/// A hash of a vector's components, bit for bit, stable on every
/// platform: XXH3-64 of their bits, each little-endian (RULES.md D460).
pub fn vector_hash(vector: &[f32]) -> u64 {
    use std::hash::Hasher as _;
    let mut hasher = crate::hash::xxhash::Xxh3Hasher::default();
    for x in vector {
        hasher.write_u32(x.to_bits());
    }
    hasher.finish()
}

/// \[DESIGN\] The HNSW layer of an entry (Malkov and Yashunin, §4,
/// Algorithm 1): floor(-ln(U) / ln(M)), U uniform in (0, 1], drawn from the
/// index's seed and the entry itself. So an entry gets one layer however
/// often it is inserted, as after reopening, and no generator state need
/// be kept. A node reaches layer l when U is at most M^-l.
pub fn level_of(seed: u64, row: u64, hash: u64, m: usize) -> u8 {
    let mut rng = Rng::new(crate::hash::xxhash::xxh3_64_words(&[seed, row, hash], 0));
    let u = rng.unit();
    let shrink = 1.0_f64 / f64::from(u32::try_from(m.max(MIN_M)).unwrap_or(u32::MAX));
    let mut level = 0_u8;
    let mut bound = shrink;
    while level < MAX_LEVEL && u <= bound {
        level = level.saturating_add(1);
        bound *= shrink;
    }
    level
}
