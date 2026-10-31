//! \[DESIGN\] Read-ahead (RULES.md D30): pages a sequential scan will read
//! next brought into the pool before it asks for them, several in one read
//! of the file (`Pager::read_ahead`), so a scan of more pages than the pool
//! holds makes one read of the file where it would make many. Split from
//! pool.rs (RULES.md R-SIZE-2).
//!
//! It is bounded: at most a `READ_AHEAD_SHARE`th of the pool's frames at
//! once, so a scan never takes the pool from the pages others use. It
//! starts only on a miss, so a scan reading pages read ahead makes no read
//! of its own until it passes them. Pages in the pool already are left as
//! they are, and it never fails the read it serves: a frame it cannot
//! have, or a page it cannot take whole, it leaves to that read. Pages read
//! ahead count as `PoolCounts::read_ahead`, not as misses; reading them
//! later is a hit.

use std::sync::PoisonError;
use std::sync::atomic::Ordering;

use super::Claim;
use crate::disk::buffer::{BufferPool, FileId, Key, Pin, count};
use crate::disk::pager::PageId;
use crate::memory::Undoing;

/// \[DESIGN\] The share of a pool's frames one read-ahead may fill: an
/// eighth, so a scan reading ahead leaves the rest to everything else.
const READ_AHEAD_SHARE: usize = 8;

/// \[DESIGN\] The pages a pool reads ahead at once, unless told otherwise
/// (`BufferPool::set_read_ahead`), as a database tells its pools its
/// setting STORAGE.READ_AHEAD_PAGES, of which this is the default: 128 KiB
/// of 4 KiB pages, one read of the file for as many pages.
pub const READ_AHEAD_PAGES: usize = 32;

impl BufferPool {
    /// Reads ahead the pages of pager file `file` from `first`, if page
    /// `first` is not in the pool: those of the next `read_ahead_pages`
    /// not in it, at most a `READ_AHEAD_SHARE`th of its frames, each into
    /// a frame of its own, unpinned. Nothing during an undo, which reads a
    /// few pages, never in order. The pages read.
    pub fn read_ahead(&self, file: FileId, first: u64) -> usize {
        let Ok(pager) = self.pager(file) else { return 0 };
        let count = self.read_ahead_pages();
        let most = count.min(self.frames.len().checked_div(READ_AHEAD_SHARE).unwrap_or_default());
        if most == 0 || Undoing::active() || self.mapped(Key { file, id: first }) {
            return 0;
        }
        let claims = self.claim_ahead(file, first, most);
        let ids: Vec<PageId> = claims.iter().map(|&(id, _)| PageId(id)).collect();
        let contents = pager.read_ahead(&ids).unwrap_or_default();
        let mut read = 0_usize;
        let mut contents = contents.into_iter();
        for (id, claim) in claims {
            let key = Key { file, id };
            match contents.next().flatten() {
                Some(content) => {
                    self.filled_ahead(key, claim, content);
                    read = read.saturating_add(1);
                }
                None => self.restore_empty(claim.frame, key),
            }
        }
        read
    }

    /// The pages it reads ahead at once: `READ_AHEAD_PAGES` unless set.
    pub fn read_ahead_pages(&self) -> usize {
        self.read_ahead_pages.load(Ordering::Relaxed)
    }

    /// Reads ahead `pages` at once from now on; 0 reads none ahead.
    pub fn set_read_ahead(&self, pages: usize) {
        self.read_ahead_pages.store(pages, Ordering::Relaxed);
    }

    /// Whether `key` is mapped to a frame.
    fn mapped(&self, key: Key) -> bool {
        self.shared_map(key).is_ok_and(|map| map.contains_key(&key))
    }

    /// Frames claimed, and emptied, for the pages of `file` from `first`,
    /// `most` of them, that are not in the pool: up to the first that
    /// cannot be had.
    fn claim_ahead(&self, file: FileId, first: u64, most: usize) -> Vec<(u64, Claim)> {
        let mut claims = Vec::with_capacity(most);
        for id in (first..).take(most) {
            let key = Key { file, id };
            let claim = match self.claim(key) {
                Ok(Some(claim)) => claim,
                Ok(None) => continue,
                Err(_) => break,
            };
            // What it held, written back; if that fails, the frame is put
            // back as it was, and the read ahead stops here.
            if self.emptied(key, claim).is_err() {
                break;
            }
            claims.push((id, claim));
        }
        claims
    }

    /// Fills `claim`'s frame, emptied, with `content`, page `key`'s, and
    /// lets it go.
    fn filled_ahead(&self, key: Key, claim: Claim, content: Vec<u8>) {
        let Ok(frame) = self.frame(claim.frame) else { return };
        *frame.content.write().unwrap_or_else(PoisonError::into_inner) = content;
        frame.dirty.store(false, Ordering::Release);
        frame.ready();
        self.eviction.accessed(claim.frame);
        self.hints.note(key, claim.frame);
        count(&self.counters.read_ahead);
        drop(Pin(frame));
    }
}

// Unit tests: an undo reads nothing ahead (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use super::*;
    use crate::disk::buffer::Clock;
    use crate::disk::encoding::Pipeline;
    use crate::disk::memory::MemoryFileSystem;
    use crate::disk::pager::Pager;
    use crate::hash::HashFunction;
    use crate::hash::crc::{CRC32_ISCSI, Crc};

    /// Verifies: [DESIGN] RULES.md D30, D38: while its thread undoes work, a
    /// pool reads nothing ahead; then it does.
    #[test]
    fn an_undo_reads_nothing_ahead() {
        let hash: Arc<dyn HashFunction> = Arc::new(Crc::new(CRC32_ISCSI).unwrap());
        let pager = Arc::new(Pager::create(&MemoryFileSystem::new(), Path::new("/p"), hash, Pipeline::none(), 4096).unwrap());
        let writer = BufferPool::new(1, Box::new(Clock::new(1))).unwrap();
        let written = writer.add_pager(Arc::clone(&pager));
        let id = writer
            .new_page(written)
            .map(|(id, mut page)| {
                page.push(1);
                id.0
            })
            .unwrap();
        writer.flush().unwrap();
        let pool = BufferPool::new(8, Box::new(Clock::new(8))).unwrap();
        let file = pool.add_pager(pager);
        let undoing = Undoing::begin();
        assert_eq!(pool.read_ahead(file, id), 0);
        drop(undoing);
        assert_eq!(pool.read_ahead(file, id), 1);
    }
}
