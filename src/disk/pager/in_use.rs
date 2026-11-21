//! \[DESIGN\] Which pages are in use, read with no lock (RULES.md D591): a
//! bit for each page, set as it is allocated and cleared as it is freed,
//! under the free list's lock, so it says what the list says. A read or
//! write of a page checks its bit, so the pager's reads take no lock of
//! the file's.
//!
//! The bits are in chunks that double in size, each made as the first of
//! its pages is allocated: the first covers `FIRST` pages, the next twice
//! as many from where it ends, and so on, so a file of any size takes a
//! few chunks and its bits no more than twice its pages.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{Extent, PageId, Pager};
use crate::error::{Code, Result, SqlError};
use crate::sync::lock_builtin;

/// \[DESIGN\] The pages of the first chunk: 4 KiB of bits.
const FIRST: u64 = 1 << 15;
/// Chunks enough for every page number.
const CHUNKS: usize = 50;
/// Bits of a word.
const WORD_BITS: u64 = 64;

/// The bits of a file's pages, by chunk.
pub(super) struct InUse {
    chunks: Box<[OnceLock<Box<[AtomicU64]>>]>,
}

/// The chunk of page `id`, and the page's place in it.
fn place(id: u64) -> (usize, u64) {
    let within = id.checked_div(FIRST).unwrap_or_default().saturating_add(1);
    let chunk = 63_u32.saturating_sub(within.leading_zeros());
    let start = FIRST.saturating_mul((1_u64 << chunk).saturating_sub(1));
    (usize::try_from(chunk).unwrap_or(usize::MAX), id.saturating_sub(start))
}

impl InUse {
    /// No page in use.
    pub(super) fn new() -> InUse {
        InUse { chunks: (0..CHUNKS).map(|_| OnceLock::new()).collect() }
    }

    /// Whether page `id` is in use.
    pub(super) fn has(&self, id: u64) -> bool {
        let (chunk, at) = place(id);
        let Some(words) = self.chunks.get(chunk).and_then(OnceLock::get) else { return false };
        let word = usize::try_from(at.checked_div(WORD_BITS).unwrap_or_default()).unwrap_or(usize::MAX);
        words.get(word).is_some_and(|w| w.load(Ordering::Acquire) & bit(at) != 0)
    }

    /// Marks the pages of `extents` in use, or not, as `used` says.
    pub(super) fn mark(&self, extents: &[Extent], used: bool) {
        for e in extents {
            let end = e.start.0.saturating_add(e.count);
            let mut id = e.start.0;
            while id < end {
                id = self.mark_word(id, end, used);
            }
        }
    }

    /// Marks the pages from `id` to `end`, or to the end of `id`'s word if
    /// that comes first: where the next word begins.
    fn mark_word(&self, id: u64, end: u64, used: bool) -> u64 {
        let (chunk, at) = place(id);
        let word_end = at.saturating_sub(at.checked_rem(WORD_BITS).unwrap_or_default()).saturating_add(WORD_BITS);
        let count = word_end.saturating_sub(at).min(end.saturating_sub(id));
        let mask = (at..at.saturating_add(count)).fold(0_u64, |m, page| m | bit(page));
        let next = id.saturating_add(count);
        let Some(slot) = self.chunks.get(chunk) else { return next };
        let word = usize::try_from(at.checked_div(WORD_BITS).unwrap_or_default()).unwrap_or(usize::MAX);
        if used {
            let size = FIRST.saturating_mul(1_u64 << chunk).checked_div(WORD_BITS).unwrap_or_default();
            let words = slot.get_or_init(|| (0..size).map(|_| AtomicU64::new(0)).collect());
            if let Some(w) = words.get(word) {
                w.fetch_or(mask, Ordering::Release);
            }
        } else if let Some(w) = slot.get().and_then(|words| words.get(word)) {
            w.fetch_and(!mask, Ordering::Release);
        }
        next
    }
}

impl Pager {
    /// Takes `count` pages from the free list, marked in use.
    pub(super) fn allocated(&self, count: u64) -> Vec<Extent> {
        let mut free = lock_builtin(&self.free);
        let extents = free.allocate(count);
        self.in_use.mark(&extents, true);
        extents
    }

    /// Returns `extents` to the free list, no longer in use.
    pub(super) fn freed(&self, extents: &[Extent]) -> std::result::Result<(), String> {
        let mut free = lock_builtin(&self.free);
        free.free(extents)?;
        self.in_use.mark(extents, false);
        Ok(())
    }

    /// Checks that page `id` is in use, as `read` and `write` need, by its
    /// bit: the free list's lock taken only to say why not.
    pub(super) fn check_in_use(&self, id: PageId) -> Result<()> {
        if !self.in_use.has(id.0) {
            let free = lock_builtin(&self.free);
            return Err(SqlError::new(
                Code::Api,
                format!("[DESIGN] page {} of {} is not allocated: its file has {} pages", id.0, self.path.display(), free.pages()),
            ));
        }
        Ok(())
    }
}

/// The bit of the page at `at` within its word.
fn bit(at: u64) -> u64 {
    1_u64 << at.checked_rem(WORD_BITS).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extent(start: u64, count: u64) -> Extent {
        Extent { start: PageId(start), count }
    }

    /// Verifies: [DESIGN] RULES.md D591: pages marked in use read so, and no
    /// others, across words and chunks, and freed read so again; a chunk
    /// never marked has no page in use.
    #[test]
    fn pages_marked_read_in_use_across_words_and_chunks() {
        let bits = InUse::new();
        let edge = FIRST - 3;
        bits.mark(&[extent(60, 10), extent(edge, 10), extent(FIRST * 3 - 1, 2)], true);
        for id in [60, 63, 64, 69, edge, FIRST, edge + 9, FIRST * 3 - 1, FIRST * 3] {
            assert!(bits.has(id), "{id}");
        }
        for id in [0, 59, 70, edge - 1, edge + 10, FIRST * 3 + 1, FIRST * 7, u64::MAX] {
            assert!(!bits.has(id), "{id}");
        }
        bits.mark(&[extent(62, 4), extent(FIRST * 3, 1)], false);
        assert!(bits.has(61) && !bits.has(62) && !bits.has(65) && bits.has(66));
        assert!(bits.has(FIRST * 3 - 1) && !bits.has(FIRST * 3));
        assert_eq!((place(0), place(FIRST), place(FIRST * 3)), ((0, 0), (1, 0), (2, 0)));
    }
}
