//! \[DESIGN\] A structure's own count of what it reads through the pool
//! (RULES.md D30, D35): its lookups, and the pages they read that the pool
//! missed, so the planner prices a fetch, and an index's probe, by how
//! much of what each table and index reads the pool holds, not by its
//! file's share of the pool. A lookup is counted on a stripe by thread,
//! each on its own cache lines, as the pool's hits are (`hits.rs`); a miss
//! reads a file anyway. A miss is counted only when the pool had no frame
//! empty and evicted a page for it: the first read of a page into a pool
//! still filling says nothing of whether the pool holds the structure, and
//! counted, a statistics pass after a start priced every fetch as a miss
//! (RULES.md D1230).

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
    #[inline]
    pub fn lookup(&self) {
        if let Some(stripe) = self.stripe() {
            count(&stripe.lookups);
        }
    }

    /// Counts a page a lookup read that the pool missed, another page
    /// evicted for it (RULES.md D1230).
    fn missed(&self) {
        if let Some(stripe) = self.stripe() {
            count(&stripe.misses);
        }
    }

    /// The pages missed per lookup, at most 1, once at least `least`
    /// lookups are counted; none before.
    pub fn missed_share(&self, least: u64) -> Option<f64> {
        self.missed_per_lookup(least).map(|share| share.min(1.0_f64))
    }

    /// The pages missed per lookup, once at least `least` lookups are
    /// counted; none before. Past `WINDOW` lookups the counts are halved,
    /// a lookup counted meanwhile kept.
    pub fn missed_per_lookup(&self, least: u64) -> Option<f64> {
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
        (lookups >= least).then_some(share)
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
        self.read_tallied_at((file, id), BufferPool::page_hash(file, id), tally)
    }

    /// As `read_tallied`, of the page whose hash is `hash` (`page_hash`).
    pub(super) fn read_tallied_at(&self, (file, id): (FileId, u64), hash: u64, tally: Option<&Tally>) -> Result<PageRef<'_>> {
        let (page, missed) = self.read_noting_at(Key { file, id }, hash)?;
        if missed && let Some(tally) = tally {
            tally.missed();
        }
        Ok(page)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `count` pages of a pager file of their own on `fs`, written back,
    /// so a pool they are added to reads each from the file; and their
    /// numbers.
    fn written(
        fs: &crate::disk::memory::MemoryFileSystem,
        name: &str,
        count: usize,
    ) -> (std::sync::Arc<crate::disk::pager::Pager>, Vec<u64>) {
        let crc = std::sync::Arc::new(crate::hash::crc::Crc::new(crate::hash::crc::CRC32_ISCSI).unwrap());
        let pipeline = crate::disk::encoding::Pipeline::none();
        let pager = std::sync::Arc::new(crate::disk::pager::Pager::create(fs, std::path::Path::new(name), crc, pipeline, 4096).unwrap());
        let frames = count.saturating_add(1);
        let pool = BufferPool::new(frames, Box::new(super::super::Clock::new(frames))).unwrap();
        let file = pool.add_pager(std::sync::Arc::clone(&pager));
        let ids = (0..count)
            .map(|n| {
                let (id, mut page) = pool.new_page(file).unwrap();
                page.extend_from_slice(&n.to_le_bytes());
                id.0
            })
            .collect();
        pool.flush().unwrap();
        (pager, ids)
    }

    /// The share of `reads` of the pages of `file`, read by `tally` as
    /// lookups of one page each, that `pool` missed and evicted for.
    fn missed_reading(pool: &BufferPool, file: FileId, tally: &Tally, reads: impl Iterator<Item = u64>) -> Option<f64> {
        for id in reads {
            tally.lookup();
            drop(pool.read_tallied(file, id, Some(tally)).unwrap());
        }
        tally.missed_share(1_u64)
    }

    /// Verifies: [DESIGN] RULES.md D1230, D35: a tally counts a page read
    /// into an empty frame as no miss, so a structure the pool holds is
    /// all held from its first reads; once no frame is empty, each miss
    /// evicts and is counted: of a structure first read into a pool full
    /// of another's pages, and of one larger than the pool. Regression
    /// test (R-TEST-7): a statistics pass into a pool just started counted
    /// its first reads as misses, and fetches of a table the pool holds
    /// were priced as misses until the tally's window forgot them.
    #[test]
    fn a_tally_counts_the_misses_that_evict() {
        let fs = crate::disk::memory::MemoryFileSystem::new();
        let (held, other, large) = (written(&fs, "/held", 8), written(&fs, "/other", 8), written(&fs, "/large", 32));
        let pool = BufferPool::new(8, Box::new(super::super::Clock::new(8))).unwrap();
        let files = [pool.add_pager(held.0), pool.add_pager(other.0), pool.add_pager(large.0)];
        let tallies = [Tally::default(), Tally::default(), Tally::default()];
        let twice = held.1.iter().chain(&held.1).copied();
        assert_eq!(missed_reading(&pool, files[0], &tallies[0], twice), Some(0.0_f64), "read into empty frames, then held");
        assert_eq!(
            missed_reading(&pool, files[1], &tallies[1], other.1.iter().copied()),
            Some(1.0_f64),
            "each first read evicts the other's"
        );
        let cycled = large.1.iter().chain(&large.1).copied();
        assert!(missed_reading(&pool, files[2], &tallies[2], cycled).is_some_and(|share| share > 0.9_f64), "larger than the pool");
    }

    /// Verifies: [DESIGN] RULES.md D35: a tally gives no share before the
    /// lookups asked for; then its misses per lookup, as a share at most 1;
    /// and past its window it halves its counts, keeping the share.
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
        assert_eq!(tally.missed_per_lookup(2_u64), Some(2.5_f64), "each lookup's pages missed");
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
