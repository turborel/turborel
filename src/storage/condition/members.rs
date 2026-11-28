//! \[DESIGN\] (RULES.md D1294) The keys a hash join's runtime filter
//! holds, as a scan is told them (`Conjunct::Among`): of an exact key,
//! the filter's bitmap of a dense range, its set of a few keys, or its
//! blocked Bloom filter of their hashes (`exec/table/runtime.rs`). A
//! column store's scan asks it of each integer of the key's column, once
//! a run where the column is kept as runs, before it decodes the other
//! columns (`engine/columns/judged.rs`), so a run of which the filter
//! keeps few rows is decoded at those rows alone.
//!
//! A bitmap and a key set say exactly whether a built key equals an
//! integer. A Bloom filter may say so of an integer no built key equals.
//! A scan that leaves a row out is right either way, as no built row
//! matches a row these do not hold. A row a Bloom filter keeps wrongly is
//! judged again by the join's own filter step or its probe, which stay
//! where they were, so the scan's answer only spares work.
//!
//! The Bloom filter is asked the hash the join made of its keys: of a
//! whole number below 2^51 in magnitude, XXH3-64, by the join's seed, of
//! the bits of the `f64` equal to it (`exec/table/join/keys.rs`). An
//! integer of another scale, or one too large, is not asked and is kept.
//!
//! It counts the rows the scan asked it of in its first runs and those it
//! kept. A run of which the scan keeps one row in a few or more is given
//! whole (`rows/frozen/batches/run.rs`), and asking the filter of it
//! spared nothing. So once those first runs are counted the scan asks it
//! only while it kept less than that share of them.

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::hash::xxhash::Words;
use crate::kernels::portable::bloom::may_hold;

/// \[DESIGN\] 1.5 × 2^52, to which a whole number below 2^51 in magnitude
/// is added in a double's bits to give the `f64` equal to it, as the join
/// hashes its keys (`exec/table/join/keys.rs`).
const MAGIC: f64 = 6_755_399_441_055_744.0_f64;

/// \[DESIGN\] A whole number below this in magnitude is its `f64` exactly
/// by `MAGIC`: 2^51.
const MAGIC_UNITS: u64 = 1_u64 << 51_u32;

/// \[DESIGN\] The bits of a word of a bitmap.
const WORD_BITS: usize = 64;

/// How the keys are held.
#[derive(Debug)]
pub enum Held {
    /// A bit of each value from `least` on, set if a built key is it.
    Bits {
        /// The least built key.
        least: i64,
        /// The bits, 64 a word.
        words: Vec<u64>,
    },
    /// The distinct built keys, ascending.
    Keys(Vec<i64>),
    /// The words of a blocked Bloom filter of the keys' hashes, and the
    /// hash of a word by the join's seed.
    Bloom {
        /// The filter's words.
        words: Vec<u64>,
        /// XXH3-64 of a word by the join's seed.
        hash: Words,
    },
}

/// \[DESIGN\] (RULES.md D1294) The keys of a hash join's runtime filter a
/// scan is told, units of `scale`, and what the scan's first runs kept.
#[derive(Debug)]
pub struct Members {
    held: Held,
    /// The scale of the built keys' units.
    scale: u32,
    /// The runs a scan counts before it may stop asking.
    tried: usize,
    runs: AtomicUsize,
    asked: AtomicUsize,
    kept: AtomicUsize,
}

impl Members {
    /// The keys `held`, units of `scale`, a scan's first `tried` runs
    /// counted.
    pub fn new(held: Held, scale: u32, tried: usize) -> Members {
        Members { held, scale, tried, runs: AtomicUsize::new(0), asked: AtomicUsize::new(0), kept: AtomicUsize::new(0) }
    }

    /// Whether a built key may equal the integer `u` of `scale`: exactly,
    /// of a bitmap or a key set; of a Bloom filter, true of every integer
    /// a built key equals and of few others. True of an integer it cannot
    /// be asked of, one of another scale than the keys'.
    pub fn may_hold(&self, u: i64, scale: u32) -> bool {
        if scale != self.scale {
            return true;
        }
        match &self.held {
            Held::Bits { least, words } => {
                let place = u.checked_sub(*least).and_then(|p| usize::try_from(p).ok());
                place.is_some_and(|p| words.get(p.div_euclid(WORD_BITS)).is_some_and(|w| (w >> (p % WORD_BITS)) & 1 == 1))
            }
            Held::Keys(keys) => keys.binary_search(&u).is_ok(),
            Held::Bloom { words, hash } => {
                if scale != 0 || u.unsigned_abs() >= MAGIC_UNITS {
                    return true;
                }
                let bits = (f64::from_bits(MAGIC.to_bits().wrapping_add(u.cast_unsigned())) - MAGIC).to_bits();
                may_hold(words, hash.hash(bits))
            }
        }
    }

    /// Whether a scan still asks it: through its first runs, and after
    /// them while it kept fewer than one in `share` of the rows it was
    /// asked of in them.
    pub fn asking(&self, share: usize) -> bool {
        if self.runs.load(Ordering::Relaxed) < self.tried {
            return true;
        }
        self.kept.load(Ordering::Relaxed).saturating_mul(share) < self.asked.load(Ordering::Relaxed)
    }

    /// Counts a run a scan asked it of, of `asked` rows of which it kept
    /// `kept`, if it is among the first.
    pub fn count(&self, asked: usize, kept: usize) {
        // Nothing is written past the first runs, so the threads of a
        // statement do not pass its counts between them for every run.
        if self.runs.load(Ordering::Relaxed) < self.tried && self.runs.fetch_add(1, Ordering::Relaxed) < self.tried {
            self.asked.fetch_add(asked, Ordering::Relaxed);
            self.kept.fetch_add(kept, Ordering::Relaxed);
        }
    }
}

// Unit tests: what each form holds, and when a scan stops asking
// (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests;
