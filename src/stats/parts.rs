//! \[DESIGN\] Parts statistics are built of, each over one column's values,
//! so an engine may feed them a row at a time or a column at a time
//! (RULES.md D31):
//!
//! * `HyperLogLog`: distinct values, counted by a sketch (Flajolet, Fusy,
//!   Gandouet and Meunier, "HyperLogLog: the analysis of a near-optimal
//!   cardinality estimation algorithm", AofA 2007) of 4,096 registers, a
//!   standard error of about 1.6%.
//! * `Reservoir`: a uniform sample of a column's values (Vitter's Algorithm
//!   R: "Random sampling with a reservoir", ACM TOMS 11(1), 1985), its
//!   random choices from a fixed seed, so the same values always give the
//!   same sample.
//! * `Histogram`: an equi-depth histogram of a sample.
//!
//! Values are hashed by their normalized form (`Value::norm`), so values SQL
//! compares equal count once.

use std::cmp::Ordering;
use std::hash::BuildHasher;

use crate::hash::murmur::MurmurBuildHasher;
use crate::value::{Value, compare};

/// Bits of a hash choosing a sketch's register: 4,096 registers.
const REGISTER_BITS: u32 = 12;
/// The most a register's rank can be: the hash's bits past the register's,
/// plus one.
const MAX_RANK: u32 = u64::BITS - REGISTER_BITS + 1;
/// \[DESIGN\] Values a sample keeps, by default.
pub const SAMPLE_VALUES: usize = 10_000;
/// \[DESIGN\] Buckets of a histogram, by default.
pub const BUCKETS: usize = 100;

/// How a column's values are sampled: the most a sample keeps, and the
/// buckets of the histogram drawn from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sampling {
    /// The most values a sample keeps.
    pub values: usize,
    /// The buckets of a histogram.
    pub buckets: usize,
}

impl Default for Sampling {
    fn default() -> Self {
        Sampling { values: SAMPLE_VALUES, buckets: BUCKETS }
    }
}
/// The seed of a sample's random choices.
const SEED: u64 = 0x9E37_79B9_7F4A_7C15;

/// A count of things as an `f64`; exact up to 2^32, and the most a `u32`
/// holds past it.
pub fn count(n: u64) -> f64 {
    f64::from(u32::try_from(n).unwrap_or(u32::MAX))
}

/// A HyperLogLog sketch of the distinct non-null values added to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HyperLogLog {
    registers: Vec<u8>,
}

impl Default for HyperLogLog {
    fn default() -> Self {
        HyperLogLog { registers: vec![0; 1 << REGISTER_BITS] }
    }
}

impl HyperLogLog {
    /// An empty sketch.
    pub fn new() -> HyperLogLog {
        HyperLogLog::default()
    }

    /// Adds `value`; NULL is not a value, and is not counted.
    pub fn add(&mut self, value: &Value) {
        if value.is_null() {
            return;
        }
        self.add_hash(MurmurBuildHasher::default().hash_one(value.norm()));
    }

    /// Adds a value given as `bytes`, the same bytes for the same value: a
    /// key of several values, as one.
    pub fn add_bytes(&mut self, bytes: &[u8]) {
        self.add_hash(hash_of(bytes));
    }

    /// Adds the value whose hash is `hash`.
    fn add_hash(&mut self, hash: u64) {
        let (register, rank) = placed(hash);
        if let Some(r) = self.registers.get_mut(register) {
            *r = (*r).max(rank);
        }
    }

    /// Adds every value `other` has had added, as if added to this sketch.
    pub fn merge(&mut self, other: &HyperLogLog) {
        for (r, o) in self.registers.iter_mut().zip(&other.registers) {
            *r = (*r).max(*o);
        }
    }

    /// The estimated number of distinct values: the harmonic mean of the
    /// registers, corrected by linear counting while registers are empty.
    pub fn estimate(&self) -> f64 {
        estimate_of(&self.registers)
    }
}

/// The registers of a sketch.
pub const REGISTERS: usize = 1 << REGISTER_BITS;

/// The register a value of hash `hash` falls in, and the rank it may raise
/// it to: the hash's top bits choose the register; the rest's leading
/// zeros, plus one, are the rank.
pub fn placed(hash: u64) -> (usize, u8) {
    let register = usize::try_from(hash >> (u64::BITS - REGISTER_BITS)).unwrap_or_default();
    let rank = (hash << REGISTER_BITS).leading_zeros().saturating_add(1).min(MAX_RANK);
    (register, u8::try_from(rank).unwrap_or(u8::MAX))
}

/// The hash a sketch takes of a value given as `bytes`.
pub fn hash_of(bytes: &[u8]) -> u64 {
    MurmurBuildHasher::default().hash_one(bytes)
}

/// The estimated number of distinct values of a sketch of `registers`.
pub fn estimate_of(registers: &[u8]) -> f64 {
    let m = count(u64::try_from(registers.len()).unwrap_or(u64::MAX));
    let alpha = 0.721_3_f64 / (1.0_f64 + 1.079_f64 / m);
    let sum: f64 = registers.iter().map(|&r| 0.5_f64.powi(i32::from(r))).sum();
    let raw = alpha * m * m / sum;
    let empty = count(registers.iter().map(|&r| u64::from(r == 0)).sum());
    if raw <= 2.5 * m && empty > 0.0 { m * (m / empty).ln() } else { raw }
}

/// A uniform sample of the non-null values added, at most `most`.
#[derive(Debug, Clone)]
pub struct Reservoir {
    values: Vec<Value>,
    most: usize,
    seen: u64,
    random: u64,
}

impl Default for Reservoir {
    fn default() -> Self {
        Reservoir::keeping(SAMPLE_VALUES)
    }
}

impl Reservoir {
    /// An empty sample of at most `SAMPLE_VALUES`.
    pub fn new() -> Reservoir {
        Reservoir::default()
    }

    /// An empty sample of at most `most` values.
    pub fn keeping(most: usize) -> Reservoir {
        Reservoir { values: Vec::new(), most, seen: 0, random: SEED }
    }

    /// The next of a xorshift sequence: random enough to sample by, and the
    /// same every time.
    fn next_random(&mut self) -> u64 {
        self.random ^= self.random << 13_u32;
        self.random ^= self.random >> 7_u32;
        self.random ^= self.random << 17_u32;
        self.random
    }

    /// Adds `value`; NULL is not sampled. Algorithm R: the n-th value
    /// replaces a kept one with chance `most` / n, so every value is
    /// kept with the same chance.
    pub fn add(&mut self, value: &Value) {
        if value.is_null() {
            return;
        }
        self.seen = self.seen.saturating_add(1);
        if self.values.len() < self.most {
            self.values.push(value.clone());
            return;
        }
        let pick = self.next_random().checked_rem(self.seen).unwrap_or_default();
        if let Some(kept) = usize::try_from(pick).ok().and_then(|i| self.values.get_mut(i)) {
            *kept = value.clone();
        }
    }

    /// The values kept, in ascending order.
    pub fn into_sorted(self) -> Vec<Value> {
        let mut values = self.values;
        values.sort_by(|a, b| compare(a, b).unwrap_or(Ordering::Equal));
        values
    }
}

/// An equi-depth histogram: bounds such that each bucket between two holds
/// as many of the values sampled.
#[derive(Debug, Clone, Default)]
pub struct Histogram {
    bounds: Vec<Value>,
}

/// A value as a number on a line, for interpolating within a bucket.
fn position(value: &Value) -> Option<f64> {
    match value {
        Value::Exact(d) => Some(d.to_f64()),
        Value::Approx(f) => Some(*f),
        Value::Char(_) | Value::Null => None,
    }
}

impl Histogram {
    /// The histogram of `sorted`, non-null values in ascending order, of
    /// at most `buckets` buckets.
    pub fn of_sorted(sorted: &[Value], buckets: usize) -> Histogram {
        let Some(last) = sorted.len().checked_sub(1) else { return Histogram::default() };
        let buckets = buckets.max(1).min(sorted.len());
        let bounds =
            (0..=buckets).filter_map(|b| sorted.get(b.saturating_mul(last).checked_div(buckets).unwrap_or_default()).cloned()).collect();
        Histogram { bounds }
    }

    /// The fraction of the values below `value`, if the histogram has any:
    /// whole buckets below it, and within its bucket, a share interpolated
    /// between the bounds for numbers, half for strings.
    pub fn below(&self, value: &Value) -> Option<f64> {
        let (first, last) = (self.bounds.first()?, self.bounds.last()?);
        let buckets = self.bounds.len().saturating_sub(1);
        if buckets == 0 || compare(value, first) != Some(Ordering::Greater) {
            return Some(0.0);
        }
        if compare(value, last) == Some(Ordering::Greater) {
            return Some(1.0);
        }
        // The bucket whose upper bound is the first not below the value.
        let upper = self.bounds.iter().position(|b| compare(b, value) != Some(Ordering::Less)).unwrap_or(buckets);
        let lower = upper.saturating_sub(1);
        let span = match (self.bounds.get(lower).and_then(position), self.bounds.get(upper).and_then(position), position(value)) {
            (Some(l), Some(h), Some(v)) => Some((l, h, v)),
            (None, _, _) | (_, None, _) | (_, _, None) => None,
        };
        let within = span.filter(|(l, h, _)| h > l).map_or(0.5_f64, |(l, h, v)| ((v - l) / (h - l)).clamp(0.0, 1.0));
        let index = |n: usize| count(u64::try_from(n).unwrap_or(u64::MAX));
        Some(((index(lower) + within) / index(buckets)).clamp(0.0, 1.0))
    }
}
