//! \[DESIGN\] The pager's free list (RULES.md D30), in memory: the free pages
//! as runs of consecutive pages, merged when they touch, and the number of
//! pages in the file. It does no I/O; the pager persists it on trunk pages
//! at `sync` and loads it on open.
//!
//! Allocation uses free pages before it grows the file, so the file stays
//! as small as its content allows, and prefers consecutive pages, so
//! overflowing content is read with few, large reads: the smallest free run
//! that holds the request; failing that, if the free pages hold it, free
//! runs largest first; failing that, every free run, the one at the end of
//! the file (if there is one) last and grown, so the new pages continue it.

use std::collections::BTreeMap;

use super::{Extent, PageId};

/// The free runs, by first page, the pages in the file, and whether either
/// changed since the list was last recorded.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct FreeList {
    runs: BTreeMap<u64, u64>,
    pages: u64,
    changed: bool,
}

impl FreeList {
    /// A file of `pages` pages, all in use.
    pub(super) fn new(pages: u64) -> FreeList {
        FreeList { runs: BTreeMap::new(), pages, changed: false }
    }

    /// Pages in the file.
    pub(super) fn pages(&self) -> u64 {
        self.pages
    }

    /// Free pages.
    pub(super) fn free_pages(&self) -> u64 {
        self.runs.values().fold(0, |sum, &count| sum.saturating_add(count))
    }

    /// The free runs, in page order.
    pub(super) fn runs(&self) -> Vec<Extent> {
        self.runs.iter().map(|(&start, &count)| Extent { start: PageId(start), count }).collect()
    }

    /// Whether page `id` is free.
    pub(super) fn is_free(&self, id: u64) -> bool {
        self.runs.range(..=id).next_back().is_some_and(|(&start, &count)| id < start.saturating_add(count))
    }

    /// Whether the list changed since `recorded` was last called.
    pub(super) fn changed(&self) -> bool {
        self.changed
    }

    /// Notes that the list as it is now is recorded.
    pub(super) fn recorded(&mut self) {
        self.changed = false;
    }

    /// Notes that the list differs from its record, which failed.
    pub(super) fn mark_changed(&mut self) {
        self.changed = true;
    }

    /// Frees `extents`, which the current record already lists as free, so
    /// the list does not differ from its record because of them.
    pub(super) fn free_recorded(&mut self, extents: &[Extent]) {
        let changed = self.changed;
        // The record took them from no list, so they cannot overlap.
        let _ = self.free(extents);
        self.changed = changed;
    }

    /// Takes `n` pages: consecutive where it can.
    pub(super) fn allocate(&mut self, n: u64) -> Vec<Extent> {
        if n == 0 {
            return Vec::new();
        }
        self.changed = true;
        if let Some((&start, _)) = self.runs.iter().filter(|&(_, &count)| count >= n).min_by_key(|&(_, &count)| count) {
            return vec![self.take(start, n)];
        }
        let enough = self.free_pages() >= n;
        let trailing =
            self.runs.iter().next_back().filter(|&(&start, &count)| !enough && start.saturating_add(count) == self.pages).map(|(&s, _)| s);
        let mut runs: Vec<(u64, u64)> = self.runs.iter().filter(|&(&start, _)| Some(start) != trailing).map(|(&s, &c)| (s, c)).collect();
        runs.sort_by_key(|&(start, count)| (std::cmp::Reverse(count), start));
        let mut extents = Vec::new();
        let mut needed = n;
        for (start, count) in runs {
            if needed == 0 {
                break;
            }
            let taken = count.min(needed);
            extents.push(self.take(start, taken));
            needed = needed.saturating_sub(taken);
        }
        if needed > 0 {
            // The free run at the end of the file, if any, grown by the rest.
            let start = trailing.map_or(self.pages, |t| {
                self.runs.remove(&t);
                t
            });
            self.pages = start.saturating_add(needed);
            extents.push(Extent { start: PageId(start), count: needed });
        }
        extents
    }

    /// Takes the first `n` pages of the run at `start`.
    fn take(&mut self, start: u64, n: u64) -> Extent {
        if let Some(count) = self.runs.remove(&start)
            && count > n
        {
            self.runs.insert(start.saturating_add(n), count.saturating_sub(n));
        }
        Extent { start: PageId(start), count: n }
    }

    /// Returns `extents` to the free list.
    ///
    /// # Errors
    ///
    /// Why, if an extent is empty, reaches past the file, or overlaps a
    /// page already free; nothing is freed then.
    pub(super) fn free(&mut self, extents: &[Extent]) -> Result<(), String> {
        let mut after = self.clone();
        for e in extents {
            after.free_one(*e)?;
        }
        after.changed = after.changed || !extents.is_empty();
        *self = after;
        Ok(())
    }

    fn free_one(&mut self, e: Extent) -> Result<(), String> {
        let (start, count) = (e.start.0, e.count);
        let end = start.checked_add(count).filter(|&end| count > 0 && end <= self.pages);
        let Some(end) = end else {
            return Err(format!("{count} pages from page {start} are not within the {} pages of the file", self.pages));
        };
        let before = self.runs.range(..start).next_back().map(|(&s, &c)| (s, s.saturating_add(c)));
        let overlaps_before = before.is_some_and(|(_, before_end)| before_end > start);
        let overlaps_after = self.runs.range(start..end).next().is_some();
        if overlaps_before || overlaps_after {
            return Err(format!("pages from {start} to {} include a free page", end.saturating_sub(1)));
        }
        let mut run = (start, count);
        if let Some((before_start, before_end)) = before.filter(|&(_, before_end)| before_end == start) {
            self.runs.remove(&before_start);
            run = (before_start, before_end.saturating_sub(before_start).saturating_add(count));
        }
        if let Some(after_count) = self.runs.remove(&end) {
            run.1 = run.1.saturating_add(after_count);
        }
        self.runs.insert(run.0, run.1);
        Ok(())
    }

    /// Takes the free run that ends the file, if there is one, so that no
    /// allocation uses its pages while the file is cut to its start. The
    /// page count stays until `release_trailing`.
    pub(super) fn take_trailing(&mut self) -> Option<Extent> {
        let (&start, &count) = self.runs.iter().next_back().filter(|&(&start, &count)| start.saturating_add(count) == self.pages)?;
        self.runs.remove(&start);
        Some(Extent { start: PageId(start), count })
    }

    /// Ends what `take_trailing` began: if `run` still ends the file, the
    /// file now ends where it starts; otherwise (pages were allocated past
    /// it meanwhile) it is free again.
    pub(super) fn release_trailing(&mut self, run: Extent) {
        self.changed = true;
        if run.start.0.saturating_add(run.count) == self.pages {
            self.pages = run.start.0;
        } else {
            // It was taken from this list, so returning it cannot overlap.
            let _ = self.free_one(run);
        }
    }

    /// Grows the file to `pages` pages, the new ones free.
    pub(super) fn grow_free(&mut self, pages: u64) -> Result<(), String> {
        if pages > self.pages {
            self.changed = true;
            let old = self.pages;
            self.pages = pages;
            self.free_one(Extent { start: PageId(old), count: pages.saturating_sub(old) })?;
        }
        Ok(())
    }
}

// Unit tests: allocation and freeing, which do no I/O (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;

    fn extent(start: u64, count: u64) -> Extent {
        Extent { start: PageId(start), count }
    }

    /// Verifies: [DESIGN] RULES.md D30, free list: with no free pages, pages
    /// come from the end of the file; freed pages merge with touching runs,
    /// before and after; the smallest run that fits is used, and what is
    /// left of it stays free.
    #[test]
    fn allocation_prefers_the_smallest_run_that_fits() {
        let mut f = FreeList::new(0);
        assert_eq!(f.allocate(10), vec![extent(0, 10)]);
        assert_eq!(f.allocate(0), vec![]);
        f.free(&[extent(2, 2), extent(6, 3), extent(4, 1)]).unwrap();
        assert_eq!(f.runs(), vec![extent(2, 3), extent(6, 3)], "4 joined 2-3; 5 still in use");
        f.free(&[extent(5, 1)]).unwrap();
        assert_eq!(f.runs(), vec![extent(2, 7)], "5 joined both");
        f.free(&[extent(0, 2)]).unwrap();
        assert_eq!((f.runs(), f.free_pages()), (vec![extent(0, 9)], 9));
        let mut g = FreeList::new(20);
        g.free(&[extent(0, 5), extent(10, 3)]).unwrap();
        assert_eq!(g.allocate(3), vec![extent(10, 3)], "the smallest run that fits");
        assert_eq!(g.allocate(2), vec![extent(0, 2)]);
        assert_eq!(g.runs(), vec![extent(2, 3)]);
        assert!(g.is_free(3) && !g.is_free(5) && !g.is_free(0));
    }

    /// Verifies: [DESIGN] RULES.md D30, free list: a request no run holds is
    /// made of free runs, largest first, while the free pages hold it, even
    /// with a free run at the end of the file; when they do not, every free
    /// run is used, the one at the end last and grown, so the file grows
    /// only by what no free page could hold.
    #[test]
    fn large_requests_grow_the_last_run_or_gather_large_runs() {
        let mut f = FreeList::new(30);
        f.free(&[extent(26, 4)]).unwrap();
        assert_eq!(f.allocate(6), vec![extent(26, 6)], "the run at the end, grown");
        assert_eq!(f.pages(), 32);
        let mut h = FreeList::new(30);
        h.free(&[extent(0, 3), extent(10, 3), extent(29, 1)]).unwrap();
        assert_eq!(h.allocate(5), vec![extent(0, 3), extent(10, 2)], "enough free pages: the file does not grow");
        assert_eq!(h.allocate(4), vec![extent(12, 1), extent(29, 3)], "not enough: the run at the end, last, grown");
        assert_eq!((h.pages(), h.free_pages()), (32, 0));
        let mut g = FreeList::new(50);
        g.free(&[extent(0, 8), extent(10, 12), extent(30, 2)]).unwrap();
        assert_eq!(g.allocate(25), vec![extent(10, 12), extent(0, 8), extent(30, 2), extent(50, 3)]);
        assert_eq!((g.runs(), g.pages()), (vec![], 53), "every free page used first");
    }

    /// Verifies: [DESIGN] RULES.md D30, free list: freeing an empty extent,
    /// one past the end of the file, or one overlapping a free page (a double
    /// free, from before or after) is refused and frees nothing; the free
    /// run that ends the file is taken while the file is cut, then the file
    /// ends at its start, unless pages were allocated past it meanwhile, when
    /// it is free again; growing adds free pages.
    #[test]
    fn invalid_frees_are_refused_and_the_end_is_trimmed() {
        let mut f = FreeList::new(10);
        f.free(&[extent(4, 2)]).unwrap();
        for bad in [extent(1, 0), extent(9, 2), extent(5, 1), extent(3, 2), extent(0, 10)] {
            assert!(f.free(&[extent(0, 1), bad]).is_err(), "{bad:?}");
            assert_eq!(f.runs(), vec![extent(4, 2)], "{bad:?}: nothing freed");
        }
        f.free(&[extent(6, 4)]).unwrap();
        let trailing = f.take_trailing().unwrap();
        assert_eq!((trailing, f.runs(), f.pages()), (extent(4, 6), vec![], 10));
        assert_eq!(f.allocate(2), vec![extent(10, 2)], "the taken run is not allocated");
        f.release_trailing(trailing);
        assert_eq!((f.runs(), f.pages()), (vec![extent(4, 6)], 12), "free again: pages were allocated past it");
        f.free(&[extent(10, 2)]).unwrap();
        let last = f.take_trailing().unwrap();
        f.release_trailing(last);
        assert_eq!((f.runs(), f.pages(), f.take_trailing()), (vec![], 4, None));
        f.grow_free(7).unwrap();
        assert_eq!((f.pages(), f.runs()), (7, vec![extent(4, 3)]));
        f.grow_free(5).unwrap();
        assert_eq!(f.pages(), 7, "never shrinks");
    }
}
