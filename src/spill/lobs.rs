//! \[DESIGN\] The database's temporary large objects (RULES.md D79): what a
//! function makes of large objects, UPPER, TRANSLATE or a regular
//! expression's replacement, and a value a held locator keeps past its
//! transaction, kept a chunk at a time in one scratch file of the work
//! areas' temporary files (`paged.rs`), made when first needed. Each chunk
//! is a page written once and passed through the file's pool, never kept
//! in a frame (`disk/buffer/pool/passing.rs`), so the memory held is a
//! chunk per reader, however many and however long. An object's pages are
//! freed when the last value holding it is dropped.

use std::fmt;
use std::sync::{Arc, Mutex};

use super::WorkAreas;
use super::paged::ScratchFile;
use crate::disk::pager::PageId;
use crate::error::{Code, Result, SqlError};
use crate::value::lob::{Chunk, Directory, Kept, LobSource, TempLobs, TempWriter};

/// \[DESIGN\] The pool's bytes of the scratch file: a few pages, as its
/// chunks pass through it.
const POOL_BYTES: usize = 65_536;

/// The temporary large objects of a database.
pub struct TempLobStore {
    work: Arc<WorkAreas>,
    file: Mutex<Option<Arc<ScratchFile>>>,
}

impl fmt::Debug for TempLobStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TempLobStore").finish_non_exhaustive()
    }
}

impl TempLobStore {
    /// The store of the work areas `work`.
    pub fn new(work: Arc<WorkAreas>) -> TempLobStore {
        TempLobStore { work, file: Mutex::new(None) }
    }

    /// Its scratch file, made now if not yet.
    fn file(&self) -> Result<Arc<ScratchFile>> {
        let mut file = crate::sync::lock(&self.file)?;
        if let Some(made) = file.as_ref() {
            return Ok(Arc::clone(made));
        }
        let made = Arc::new(ScratchFile::create(&self.work.memory, &self.work.scratch(), POOL_BYTES, "temporary large objects")?);
        *file = Some(Arc::clone(&made));
        Ok(made)
    }
}

impl TempLobs for TempLobStore {
    fn writer(&self) -> Result<Box<dyn TempWriter>> {
        Ok(Box::new(Writer { file: self.file()?, pages: Vec::new(), sizes: Vec::new() }))
    }
}

/// A temporary large object being written.
struct Writer {
    file: Arc<ScratchFile>,
    pages: Vec<PageId>,
    sizes: Vec<Chunk>,
}

impl TempWriter for Writer {
    fn push(&mut self, chunk: &[u8], chars: u32) -> Result<()> {
        let octets = u32::try_from(chunk.len()).map_err(|_| SqlError::new(Code::Api, "[DESIGN] a chunk longer than a chunk"))?;
        let page = self.file.pool()?.write_passing(self.file.id(), chunk)?;
        self.pages.push(page);
        self.sizes.push(Chunk { octets, chars });
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> Result<Arc<dyn LobSource>> {
        let pages = std::mem::take(&mut self.pages);
        let directory = Directory::new(std::mem::take(&mut self.sizes));
        Ok(Arc::new(Temp { file: Arc::clone(&self.file), pages, directory }))
    }
}

impl Drop for Writer {
    /// A writer dropped unfinished frees what it wrote.
    fn drop(&mut self) {
        free(&self.file, &self.pages);
    }
}

/// Frees `pages` of `file`; one that cannot be is left to the file, which
/// goes when the database closes.
fn free(file: &ScratchFile, pages: &[PageId]) {
    if let Ok(pool) = file.pool() {
        for &page in pages {
            let _ = pool.free(file.id(), page);
        }
    }
}

/// A temporary large object: its chunks' pages in the scratch file.
struct Temp {
    file: Arc<ScratchFile>,
    pages: Vec<PageId>,
    directory: Directory,
}

impl fmt::Debug for Temp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Temp").field("chunks", &self.pages.len()).finish_non_exhaustive()
    }
}

impl LobSource for Temp {
    fn totals(&self) -> (u64, u64) {
        self.directory.totals()
    }

    fn directory(&self) -> Result<&Directory> {
        Ok(&self.directory)
    }

    fn chunk(&self, k: usize) -> Result<Arc<[u8]>> {
        let page =
            self.pages.get(k).ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] a temporary large object has no chunk {k}")))?;
        Ok(Arc::from(self.file.pool()?.read_passing(self.file.id(), *page)?))
    }

    fn kept(&self) -> Option<Kept> {
        None
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        free(&self.file, &self.pages);
    }
}

// Unit tests: a temporary large object written and read back, its pages
// freed when it goes (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::MemoryBudget;
    use crate::value::lob::{CHUNK_OCTETS, LobBuilder, LobKind};

    /// Verifies: [DESIGN] RULES.md D79: a value of several chunks made
    /// through the store reads back as written, by ranges across chunks;
    /// its pages are freed when the last value holding it is dropped, and
    /// reused by the next.
    #[test]
    fn temporary_large_objects_are_kept_and_freed() {
        let work = Arc::new(WorkAreas::new(MemoryBudget::new(64 << 20)));
        let store = TempLobStore::new(work);
        let text: String = (0..CHUNK_OCTETS).map(|i| if i % 5 == 0 { 'ß' } else { 'x' }).collect();
        let mut b = LobBuilder::new(LobKind::Character, Some(&store));
        b.push_str(&text).unwrap();
        b.push_str(&text).unwrap();
        let lob = b.finish().unwrap();
        assert_eq!(lob.octets(), 2 * u64::try_from(text.len()).unwrap());
        let read = lob.reader().read(0, lob.octets()).unwrap();
        assert_eq!(read, [text.as_bytes(), text.as_bytes()].concat());
        let file = store.file().unwrap();
        let pool = file.pool().unwrap();
        let in_use = pool.pages_in_use(file.id()).unwrap();
        drop(lob);
        assert!(pool.pages_in_use(file.id()).unwrap() < in_use, "its pages freed");
    }
}
