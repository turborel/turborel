//! \[DESIGN\] A chunk directory: the pages of an array of fixed-width
//! records, node records or compressed codes, in order. The paper finds
//! node i by offset arithmetic in one file [DiskANN, NeurIPS 2019 §3.2];
//! pages of a shared pager file need not be contiguous, so the directory
//! adds one lookup in memory. It is kept in a chain of directory pages,
//! appended to as chunks are added and read whole when the index opens.
//!
//! A directory page is a tag, the next directory page, a count, then
//! that many chunk page numbers.

use crate::bytes::{Put, Reader};
use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::pager::PageId;
use crate::error::Result;

use super::node::corrupt;

/// The tag of a directory page.
const TAG: u8 = 68;
/// Tag, next page and count before the chunk page numbers.
const DIRECTORY_HEADER: usize = 13;
/// Bytes of a page number.
const PAGE_NUMBER: usize = 8;
/// No next directory page; the pager never allocates this number.
pub(super) const NO_PAGE: u64 = u64::MAX;

/// A directory, as read into memory.
#[derive(Debug, Clone, Default)]
pub(super) struct Chunks {
    /// The chunk pages, in order.
    pub(super) pages: Vec<u64>,
    /// The directory pages, the first the head the meta page names.
    pub(super) directory: Vec<PageId>,
    /// Chunk pages a directory page holds.
    per_directory: usize,
}

impl Chunks {
    /// A new empty directory: its head page.
    ///
    /// # Errors
    ///
    /// The buffer pool's.
    pub(super) fn create(pool: &BufferPool, file: FileId) -> Result<Chunks> {
        let per_directory = per_directory(pool, file)?;
        let (head, mut page) = pool.new_page(file)?;
        *page = directory_bytes(NO_PAGE, &[]);
        Ok(Chunks { pages: Vec::new(), directory: vec![head], per_directory })
    }

    /// The directory whose head is `head`.
    ///
    /// # Errors
    ///
    /// The buffer pool's; SQLCODE -916 for a malformed or cyclic chain.
    pub(super) fn open(pool: &BufferPool, file: FileId, head: PageId) -> Result<Chunks> {
        let per_directory = per_directory(pool, file)?;
        let mut chunks = Chunks { pages: Vec::new(), directory: Vec::new(), per_directory };
        let mut next = head.0;
        while next != NO_PAGE {
            if u64::try_from(chunks.directory.len()).unwrap_or(u64::MAX) > pool.pages_in_use(file)? {
                return Err(corrupt("a chunk directory's chain is cyclic"));
            }
            chunks.directory.push(PageId(next));
            let page = pool.read(file, next)?;
            let mut r = Reader::new(&page, "DiskANN chunk directory");
            if r.u8()? != TAG {
                return Err(r.corrupt("its tag is not a directory's"));
            }
            next = r.u64()?;
            let count = r.u32()?;
            for _ in 0..count {
                chunks.pages.push(r.u64()?);
            }
            r.finish()?;
        }
        Ok(chunks)
    }

    /// Adds chunk page `page`, written to the last directory page, or to a
    /// new one linked from it.
    ///
    /// # Errors
    ///
    /// The buffer pool's; the chunk is then not added.
    pub(super) fn push(&mut self, pool: &BufferPool, file: FileId, page: u64) -> Result<()> {
        let last = *self.directory.last().ok_or_else(|| corrupt("a chunk directory has no head"))?;
        let held = self.pages.len().checked_rem(self.per_directory).unwrap_or(0);
        if held == 0 && !self.pages.is_empty() {
            let (next, mut fresh) = pool.new_page(file)?;
            *fresh = directory_bytes(NO_PAGE, &[page]);
            drop(fresh);
            let from = self.pages.len().saturating_sub(self.per_directory);
            *pool.write(file, last)? = directory_bytes(next.0, self.pages.get(from..).unwrap_or_default());
            self.directory.push(next);
        } else {
            let from = self.pages.len().saturating_sub(held);
            let mut kept = self.pages.get(from..).unwrap_or_default().to_vec();
            kept.push(page);
            *pool.write(file, last)? = directory_bytes(NO_PAGE, &kept);
        }
        self.pages.push(page);
        Ok(())
    }

    /// The page of chunk `chunk`.
    pub(super) fn page(&self, chunk: usize) -> Result<u64> {
        self.pages.get(chunk).copied().ok_or_else(|| corrupt("a node is past its chunk directory"))
    }

    /// The head, which the meta page names.
    pub(super) fn head(&self) -> PageId {
        self.directory.first().copied().unwrap_or(PageId(NO_PAGE))
    }

    /// Frees every chunk and directory page.
    ///
    /// # Errors
    ///
    /// The buffer pool's; some pages may already be freed.
    pub(super) fn free(&self, pool: &BufferPool, file: FileId) -> Result<()> {
        self.pages.iter().map(|&p| PageId(p)).chain(self.directory.iter().copied()).try_for_each(|p| pool.free(file, p))
    }
}

fn per_directory(pool: &BufferPool, file: FileId) -> Result<usize> {
    let per = pool.capacity(file)?.saturating_sub(DIRECTORY_HEADER).checked_div(PAGE_NUMBER).unwrap_or(0);
    if per == 0 {
        return Err(corrupt("its pages are too small for a chunk directory"));
    }
    Ok(per)
}

fn directory_bytes(next: u64, pages: &[u64]) -> Vec<u8> {
    let mut bytes = vec![TAG];
    bytes.put_u64(next);
    bytes.put_u32(u32::try_from(pages.len()).unwrap_or(u32::MAX));
    for &p in pages {
        bytes.put_u64(p);
    }
    bytes
}
