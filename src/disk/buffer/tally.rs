//! \[DESIGN\] A structure's own count of what it reads through the pool
//! (RULES.md D30, D35): its lookups, and the pages they read that the pool
//! missed, so the planner prices a fetch by how much of what each table
//! reads the pool holds, not by its file's share of the pool. A lookup is
//! counted on a stripe by thread, each on its own cache lines, as the
//! pool's hits are (`hits.rs`); a miss reads a file anyway.

use std::sync::atomic::{AtomicU64, Ordering};

use super::{BufferPool, FileId, Key, PageRef, count};
use crate::error::Result;
use crate::sync::thread_stripe;

/// \[DESIGN\] Stripes of a tally: fewer than the pool's, as every table and
/// index has one.
const STRIPES: usize = 16;
/// \[DESIGN\] The lookups past which a tally halves its counts as it is
/// asked, so its share follows what is read now more than long ago.
const WINDOW: u64 = 1 << 16;

/// A stripe's lookups and misses, alone on its cache lines.
#[repr(align(128))]
#[derive(Default)]
struct Stripe {
    lookups: AtomicU64,
    misses: AtomicU64,
}

/// \[DESIGN\] A structure's lookups through the pool, and the pages they
/// read that it missed.
pub struct Tally {
    stripes: Box<[Stripe]>,
}

impl Default for Tally {
    fn default() -> Tally {
        Tally { stripes: (0..STRIPES).map(|_| Stripe::default()).collect() }
    }
}

impl Tally {
    /// The thread's stripe.
    fn stripe(&self) -> Option<&Stripe> {
        self.stripes.get(thread_stripe(STRIPES))
    }

    /// Counts a lookup begun.
    pub fn lookup(&self) {
        if let Some(stripe) = self.stripe() {
            count(&stripe.lookups);
        }
    }

    /// Counts a page a lookup read that the pool missed.
    fn missed(&self) {
        if let Some(stripe) = self.stripe() {
            count(&stripe.misses);
        }
    }

    /// The pages missed per lookup, at most 1, once at least `least`
    /// lookups are counted; none before. Past `WINDOW` lookups the counts
    /// are halved, a lookup counted meanwhile kept.
    pub fn missed_share(&self, least: u64) -> Option<f64> {
        let sum =
            |each: fn(&Stripe) -> &AtomicU64| self.stripes.iter().fold(0_u64, |n, s| n.saturating_add(each(s).load(Ordering::Relaxed)));
        let (lookups, misses) = (sum(|s| &s.lookups), sum(|s| &s.misses));
        if lookups > WINDOW {
            for s in &self.stripes {
                for counter in [&s.lookups, &s.misses] {
                    counter.fetch_sub(counter.load(Ordering::Relaxed) >> 1_u32, Ordering::Relaxed);
                }
            }
        }
        let share = crate::stats::parts::count(misses) / crate::stats::parts::count(lookups).max(1.0_f64);
        (lookups >= least).then_some(share.min(1.0_f64))
    }
}

impl BufferPool {
    /// Holds page or block `id` of `file` for reading, as `read` does,
    /// counting it on `tally`, if given, if the pool missed it.
    ///
    /// # Errors
    ///
    /// As `read`.
    pub fn read_tallied(&self, file: FileId, id: u64, tally: Option<&Tally>) -> Result<PageRef<'_>> {
        let (page, missed) = self.read_noting(Key { file, id })?;
        if missed && let Some(tally) = tally {
            tally.missed();
        }
        Ok(page)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D35: a tally gives no share before the
    /// lookups asked for; then its misses per lookup, at most 1; and past
    /// its window it halves its counts, keeping the share.
    #[test]
    fn a_tally_gives_its_misses_per_lookup() {
        let tally = Tally::default();
        tally.lookup();
        tally.missed();
        assert_eq!(tally.missed_share(2_u64), None, "too few lookups");
        tally.lookup();
        assert_eq!(tally.missed_share(2_u64), Some(0.5_f64));
        for _ in 0..4_u32 {
            tally.missed();
        }
        assert_eq!(tally.missed_share(2_u64), Some(1.0_f64), "at most 1");
        let many = Tally::default();
        for n in 0..=WINDOW {
            many.lookup();
            if n % 4_u64 == 0_u64 {
                many.missed();
            }
        }
        let before = many.missed_share(1_u64).unwrap();
        let after = many.missed_share(1_u64).unwrap();
        assert!((before - 0.25_f64).abs() < 1e-3_f64 && (after - 0.25_f64).abs() < 1e-3_f64, "{before} {after}");
    }
}
