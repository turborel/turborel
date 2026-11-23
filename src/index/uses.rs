//! \[DESIGN\] What INFORMATION_SCHEMA.INDEX_STATISTICS shows of an index
//! (RULES.md D522): how often it was searched and the entries the searches
//! gave, and what its method knows of its size (`Index::sizes`).
//!
//! A search counts what it reads in its own fields (`Using`) and adds them
//! to the index's counts once, as it is dropped, as a statement's rows
//! read are counted (`txn::counted`). The counts are kept in stripes, a
//! thread's own, each on a cache line of its own, so searches on many
//! threads at once do not take turns at one line; a read of them sums the
//! stripes. They are relaxed: a total read while searches run may miss
//! those not yet dropped, and is exact once they are.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::error::Result;

/// The stripes the counts are kept in.
const STRIPES: usize = 16;

/// The stripe the next thread to count takes.
static NEXT_STRIPE: AtomicUsize = AtomicUsize::new(0);

std::thread_local! {
    /// This thread's stripe.
    static STRIPE: usize = NEXT_STRIPE.fetch_add(1, Ordering::Relaxed).checked_rem(STRIPES).unwrap_or_default();
}

/// \[DESIGN\] What an index's method knows of its size, each part none if
/// it does not know it cheaply.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IndexSizes {
    /// Its entries: one for each version of each row holding a key.
    pub entries: Option<u64>,
    /// The levels of its tree, of a method keeping one: 1 while the root
    /// is a leaf.
    pub height: Option<u64>,
    /// The bytes of the pages it is on.
    pub bytes: Option<u64>,
}

/// A stripe of the counts.
#[repr(align(64))]
#[derive(Debug, Default)]
struct Stripe {
    searches: AtomicU64,
    entries: AtomicU64,
}

/// \[DESIGN\] An index's counts of searches and the entries they gave,
/// since the database was opened or the index was made.
#[derive(Debug)]
pub struct Uses {
    stripes: Vec<Stripe>,
}

impl Default for Uses {
    fn default() -> Self {
        Uses { stripes: (0..STRIPES).map(|_| Stripe::default()).collect() }
    }
}

impl Uses {
    /// Adds `searches` searches, which gave `entries` entries, to this
    /// thread's stripe.
    pub(crate) fn add(&self, searches: u64, entries: u64) {
        if let Some(stripe) = STRIPE.with(|&at| self.stripes.get(at)) {
            stripe.searches.fetch_add(searches, Ordering::Relaxed);
            stripe.entries.fetch_add(entries, Ordering::Relaxed);
        }
    }

    /// The searches and the entries they gave, the stripes summed.
    pub fn totals(&self) -> (u64, u64) {
        self.stripes.iter().fold((0, 0), |(s, e), stripe| {
            (s.saturating_add(stripe.searches.load(Ordering::Relaxed)), e.saturating_add(stripe.entries.load(Ordering::Relaxed)))
        })
    }
}

/// \[DESIGN\] The searches of one reader of an index and the entries they
/// gave, added to the index's counts as it is dropped. A reader of an
/// index no counts are kept for counts nothing.
#[derive(Debug, Default)]
pub struct Using {
    uses: Option<Arc<Uses>>,
    searches: u64,
    entries: u64,
}

impl Using {
    /// A reader of the index whose counts are `uses`, if they are kept.
    pub fn of(uses: Option<Arc<Uses>>) -> Using {
        Using { uses, searches: 0, entries: 0 }
    }

    /// It searched once more.
    pub fn searched(&mut self) {
        self.searches = self.searches.saturating_add(1);
    }

    /// Its searches gave `n` entries more.
    pub fn gave(&mut self, n: u64) {
        self.entries = self.entries.saturating_add(n);
    }

    /// Adds what it counted to the index's counts, and counts on from 0:
    /// as its last entry is taken, not once it is dropped, which may be
    /// after its statement ends.
    pub fn flush(&mut self) {
        if let Some(uses) = self.uses.as_deref().filter(|_| self.searches > 0 || self.entries > 0) {
            uses.add(std::mem::take(&mut self.searches), std::mem::take(&mut self.entries));
        }
    }
}

impl Drop for Using {
    fn drop(&mut self) {
        self.flush();
    }
}

/// \[DESIGN\] The entries of one search, `entries`, counted as they are
/// taken: the search, and each entry, added to the index's counts once
/// they are dropped, or once the last is taken.
pub struct Used<I> {
    entries: I,
    using: Using,
}

/// The entries `entries` of a search of the index whose counts are `uses`.
pub fn used<I>(entries: I, uses: Option<Arc<Uses>>) -> Used<I> {
    let mut using = Using::of(uses);
    using.searched();
    Used { entries, using }
}

/// The entries `entries` of a part of a search, the search counted once
/// for every part (`Using`), each entry counted as the index's.
pub fn part_of<I>(entries: I, uses: Option<Arc<Uses>>) -> Used<I> {
    Used { entries, using: Using::of(uses) }
}

impl<I: Iterator<Item = Result<T>>, T> Iterator for Used<I> {
    type Item = Result<T>;

    fn next(&mut self) -> Option<Result<T>> {
        let next = self.entries.next();
        match &next {
            Some(Ok(_)) => self.using.gave(1),
            Some(Err(_)) => {}
            // The last: counted now, not once dropped, which may be after
            // their statement ends.
            None => self.using.flush(),
        }
        next
    }
}

// Unit tests: the counts' arithmetic (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D522: a search counts once, with each
    /// entry it gave, when its last entry is taken or when it is dropped
    /// part read; a reader that never searched adds nothing, a part of a
    /// search its entries alone, and one of an index no counts are kept
    /// for counts nothing.
    #[test]
    fn searches_count_once_with_their_entries() {
        let uses = Arc::new(Uses::default());
        let all: Vec<Result<u8>> = used([Ok(1_u8), Ok(2_u8)].into_iter(), Some(Arc::clone(&uses))).collect();
        assert_eq!(all.len(), 2);
        assert_eq!(uses.totals(), (1, 2), "read to the end");
        let mut part = used([Ok(1_u8), Ok(2_u8), Ok(3_u8)].into_iter(), Some(Arc::clone(&uses)));
        assert!(part.next().is_some());
        assert_eq!(uses.totals(), (1, 2), "not yet dropped");
        drop(part);
        assert_eq!(uses.totals(), (2, 3), "dropped part read");
        drop(Using::of(Some(Arc::clone(&uses))));
        assert_eq!(uses.totals(), (2, 3), "never searched");
        let mut probes = Using::of(Some(Arc::clone(&uses)));
        probes.searched();
        probes.searched();
        probes.gave(5);
        drop(probes);
        assert_eq!(uses.totals(), (4, 8), "a join's probes");
        assert_eq!(part_of([Ok(1_u8)].into_iter(), Some(Arc::clone(&uses))).count(), 1);
        assert_eq!(uses.totals(), (4, 9), "a part of a search counts its entries alone");
        assert_eq!(used([Ok(1_u8)].into_iter(), None).count(), 1, "nothing kept, nothing counted");
    }
}
