//! \[DESIGN\] (RULES.md D805) The files a pool reads and writes, by their
//! number, found with no lock and no reference count moved. Every page
//! written, and every page read from its file, finds its file here. Behind
//! one reader-writer lock the lock's reader count was a cache line each
//! such call wrote, and the file's `Arc`, cloned out, another; threads
//! writing pages of different tables passed both lines between them.
//!
//! Files are only ever added, so each is set once in a slot that is never
//! changed again: slots in chunks made as files are added, a chunk set
//! once in a fixed table of chunks. A lookup reads two `OnceLock`s, loads
//! only, and borrows the file for as long as the pool lives.

use std::sync::{Mutex, OnceLock, PoisonError};

use super::{FileId, Source};

/// \[DESIGN\] Files in a chunk of slots, made as the first of them is added.
/// With `CHUNKS` it bounds what a pool holds, 1,048,576 files at most.
const CHUNK: usize = 1024;
/// \[DESIGN\] Chunks a pool may have.
const CHUNKS: usize = 1024;

/// A chunk of slots, each set once.
type Chunk = Box<[OnceLock<Source>]>;

/// A pool's files, by number.
pub(super) struct Sources {
    chunks: Box<[OnceLock<Chunk>]>,
    /// The files added, under which the next is added.
    added: Mutex<usize>,
}

impl Sources {
    pub(super) fn new() -> Sources {
        Sources { chunks: (0..CHUNKS).map(|_| OnceLock::new()).collect(), added: Mutex::new(0) }
    }

    /// Adds `source`; its number, or none if the pool has all it may hold.
    pub(super) fn add(&self, source: Source) -> Option<FileId> {
        let mut added = self.added.lock().unwrap_or_else(PoisonError::into_inner);
        let n = *added;
        let file = FileId(u32::try_from(n).ok()?);
        let chunk = self.chunks.get(n.checked_div(CHUNK)?)?;
        let slots = chunk.get_or_init(|| (0..CHUNK).map(|_| OnceLock::new()).collect());
        slots.get(n.checked_rem(CHUNK)?)?.set(source).ok()?;
        *added = n.checked_add(1)?;
        Some(file)
    }

    /// The file numbered `file`, if one was added.
    pub(super) fn get(&self, file: FileId) -> Option<&Source> {
        let n = usize::try_from(file.0).ok()?;
        self.chunks.get(n.checked_div(CHUNK)?)?.get()?.get(n.checked_rem(CHUNK)?)?.get()
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use super::*;
    use crate::disk::encoding::Pipeline;
    use crate::disk::memory::MemoryFileSystem;
    use crate::disk::pager::Pager;
    use crate::hash::HashFunction;
    use crate::hash::crc::{CRC32_ISCSI, Crc};

    fn pager(n: usize) -> Arc<Pager> {
        let hash: Arc<dyn HashFunction> = Arc::new(Crc::new(CRC32_ISCSI).unwrap());
        Arc::new(Pager::create(&MemoryFileSystem::new(), Path::new(&format!("/s{n}")), hash, Pipeline::none(), 4096).unwrap())
    }

    /// Verifies: [DESIGN] RULES.md D805: files are numbered in the order
    /// they are added, across chunks, each found as it was added and the
    /// same one every time; a number never added finds none.
    #[test]
    fn files_are_found_by_the_number_they_were_added_as() {
        let sources = Sources::new();
        let past = CHUNK.checked_add(3).unwrap();
        let pagers: Vec<Arc<Pager>> = (0..past).map(pager).collect();
        for (n, p) in pagers.iter().enumerate() {
            assert_eq!(sources.add(Source::Pages(Arc::clone(p))), Some(FileId(u32::try_from(n).unwrap())));
        }
        for (n, p) in pagers.iter().enumerate() {
            let Some(Source::Pages(found)) = sources.get(FileId(u32::try_from(n).unwrap())) else { panic!("file {n}") };
            assert!(Arc::ptr_eq(found, p), "file {n}");
        }
        assert!(sources.get(FileId(u32::try_from(past).unwrap())).is_none(), "never added");
        assert!(sources.get(FileId(u32::MAX)).is_none(), "past every chunk");
    }
}
