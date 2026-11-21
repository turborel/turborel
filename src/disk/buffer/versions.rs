//! \[DESIGN\] (RULES.md D502) The version word of each frame, read without
//! a lock: a page read under its latch is stamped with its frame and the
//! frame's version, and a copy of it is current while the frame holds the
//! page at that version. A reader that checks a copy so writes nothing
//! shared, where taking the latch would write the latch's word.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{RwLockWriteGuard, TryLockError};

use super::{BufferPool, FileId, Frame, Key, PageRef, Tally};
use crate::error::Result;

/// The pools made so far, so each has a number of its own.
static POOLS: AtomicU64 = AtomicU64::new(1);

/// A new pool's number, never one another pool of the process had.
pub(super) fn next_pool() -> u64 {
    POOLS.fetch_add(1, Ordering::Relaxed)
}

impl Frame {
    /// Its content latched exclusively, its version moved on, if no one
    /// holds its latch now.
    pub(super) fn try_latched(&self) -> Option<RwLockWriteGuard<'_, Vec<u8>>> {
        let latch = match self.content.try_write() {
            Ok(latch) => latch,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => return None,
        };
        self.version.fetch_add(1, Ordering::SeqCst);
        Some(latch)
    }
}

/// Where and when a page was read: its pool's number, its frame, and the
/// frame's version then.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    pool: u64,
    frame: usize,
    version: u64,
}

impl BufferPool {
    /// Page `id` of pager file `file`, as `read_tallied` reads it, with the
    /// stamp of the frame that holds it.
    ///
    /// # Errors
    ///
    /// As `read_tallied`.
    pub fn read_stamped(&self, file: FileId, id: u64, tally: Option<&Tally>) -> Result<(PageRef<'_>, Stamp)> {
        let page = self.read_tallied(file, id, tally)?;
        // The page is latched in its frame, so the frame's version stays
        // as it is read here.
        let frame = page.frame;
        let stamp = Stamp { pool: self.number, frame: frame.index, version: frame.version.load(Ordering::SeqCst) };
        Ok((page, stamp))
    }

    /// Whether page `id` of `file`, read with `stamp`, is as it was then:
    /// its frame not latched exclusively since, so neither changed nor
    /// given another page. A write under way when the stamp was taken
    /// moved the version on as it took the latch, before it changed a byte.
    pub fn unchanged(&self, file: FileId, id: u64, stamp: Stamp) -> bool {
        let Some(frame) = self.frames.get(stamp.frame).filter(|_| stamp.pool == self.number) else { return false };
        frame.version.load(Ordering::SeqCst) == stamp.version && frame.key() == Some(Key { file, id })
    }
}

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

    /// A pool of `frames` frames over a new pager file, and the file.
    fn pool(frames: usize) -> (BufferPool, FileId) {
        let hash: Arc<dyn HashFunction> = Arc::new(Crc::new(CRC32_ISCSI).unwrap());
        let pager = Pager::create(&MemoryFileSystem::new(), Path::new("/v"), hash, Pipeline::none(), 4096).unwrap();
        let pool = BufferPool::new(frames, Box::new(Clock::new(frames))).unwrap();
        let file = pool.add_pager(Arc::new(pager));
        (pool, file)
    }

    /// The stamp page `id` of `file` is read with.
    fn stamp(pool: &BufferPool, file: FileId, id: u64) -> Stamp {
        pool.read_stamped(file, id, None).unwrap().1
    }

    /// Verifies: [DESIGN] RULES.md D502: a page read is stamped, and is
    /// unchanged while read only; a write of it, even one that changes no
    /// byte, moves its frame's version on; a page evicted and read again,
    /// another page, and a stamp of another pool are not unchanged.
    #[test]
    fn a_stamp_holds_until_its_frame_is_latched_exclusively() {
        let (pool, file) = pool(2);
        let a = pool.new_page(file).unwrap().0;
        let b = pool.new_page(file).unwrap().0;
        let c = pool.new_page(file).unwrap().0;
        let first = stamp(&pool, file, a.0);
        drop(pool.read(file, a.0).unwrap());
        assert!(pool.unchanged(file, a.0, first), "read only");
        assert!(!pool.unchanged(file, b.0, first), "another page");
        drop(pool.write(file, a).unwrap());
        assert!(!pool.unchanged(file, a.0, first), "written");
        let second = stamp(&pool, file, a.0);
        assert!(pool.unchanged(file, a.0, second));
        for id in [b, c, b, c] {
            drop(pool.read(file, id.0).unwrap());
        }
        assert!(!pool.unchanged(file, a.0, second), "evicted");
        let (other, other_file) = self::pool(2);
        let page = other.new_page(other_file).unwrap().0;
        let theirs = stamp(&other, other_file, page.0);
        assert!(!pool.unchanged(file, a.0, theirs), "of another pool");
    }
}
