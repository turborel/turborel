//! \[DESIGN\] A bucket's chain rewritten as its entries are read, a page at a
//! time (`engine/hash.rs`): a split moves some of a bucket's entries to a new
//! bucket and keeps the rest in place, a merge appends one bucket's entries
//! after another's, and neither holds more than a page read and a page to
//! write, so a bucket of a key of more entries than memory holds splits and
//! merges as any.
//!
//! A chain of no more than `HELD_PAGES` pages, as most are, is read once,
//! its entries held, and written from them; a longer one is read again
//! for each pass, a page at a time.
//!
//! The entries kept in a chain are written over its own pages, from its
//! first: as they were each on a page read before, packed again they take
//! no more pages than those read, so a page is written only once it has
//! been read. Pages past the last written are left to be freed; pages
//! past the chain's are new.

use super::bucket::{self, Entry, fragment};
use super::{HashTable, Hashed, hash};
use crate::disk::pager::PageId;
use crate::error::Result;

/// \[DESIGN\] The pages of a chain whose entries a split or merge holds in
/// memory, read once: past them, it reads them again a page at a time.
const HELD_PAGES: usize = 8;

/// A chain being written: its pages, those written, and the entries of
/// the next page, with the bytes they take.
pub(super) struct Rewriter<'t> {
    table: &'t HashTable,
    pages: Vec<PageId>,
    written: usize,
    held: Vec<(u32, Entry)>,
    used: usize,
}

impl<'t> Rewriter<'t> {
    /// A chain of `table` to be written over `pages`, its first page
    /// staying first.
    pub(super) fn over(table: &'t HashTable, pages: Vec<PageId>) -> Rewriter<'t> {
        Rewriter { table, pages, written: 0, held: Vec::new(), used: bucket::HEADER }
    }

    /// Adds `entry`, of hash `h`, after those added: the page before written
    /// once it is full.
    ///
    /// # Errors
    ///
    /// The buffer pool's errors.
    pub(super) fn push(&mut self, h: u64, entry: Entry) -> Result<()> {
        let size = bucket::size(&entry.0, &entry.1);
        if self.used.saturating_add(size) > self.table.capacity || self.held.len() == usize::from(u16::MAX) {
            let next = self.page(self.written.saturating_add(1))?;
            self.write(Some(next))?;
        }
        self.used = self.used.saturating_add(size);
        self.held.push((fragment(h), entry));
        Ok(())
    }

    /// Writes the last page, ending the chain there: the pages left over,
    /// to be freed.
    ///
    /// # Errors
    ///
    /// The buffer pool's errors.
    pub(super) fn finish(mut self) -> Result<Vec<PageId>> {
        self.write(None)?;
        Ok(self.pages.split_off(self.written))
    }

    /// Writes the entries held over the next page, its next page `next`.
    fn write(&mut self, next: Option<PageId>) -> Result<()> {
        let id = self.page(self.written)?;
        let content: Vec<(u32, &Entry)> = self.held.iter().map(|(f, e)| (*f, e)).collect();
        *self.table.pool.write(self.table.file, id)? = bucket::build(&content, next)?;
        self.written = self.written.saturating_add(1);
        self.held.clear();
        self.used = bucket::HEADER;
        Ok(())
    }

    /// The chain's page `n`, a new one past its pages.
    fn page(&mut self, n: usize) -> Result<PageId> {
        while self.pages.len() <= n {
            self.pages.push(HashTable::new_page(&self.table.pool, self.table.file, bucket::empty())?);
        }
        self.pages.get(n).copied().ok_or_else(|| super::corrupt("a chain lost a page"))
    }
}

impl HashTable {
    /// The pages of the chain that begins at `head`, and its entries, each
    /// with its key's hash, if it has no more than `HELD_PAGES` pages.
    ///
    /// # Errors
    ///
    /// The buffer pool's errors; SQLCODE -916 if a page is in it twice.
    pub(super) fn held_chain(&self, head: PageId) -> Result<(Vec<PageId>, Option<Hashed>)> {
        let (mut pages, mut entries_read) = (Vec::new(), Some(Vec::new()));
        let mut at = Some(head);
        while let Some(p) = at {
            if pages.contains(&p) {
                return Err(super::corrupt(&format!("page {} is in its chain twice", p.0)));
            }
            let page = self.pool.read(self.file, p.0)?;
            pages.push(p);
            entries_read = entries_read.filter(|_| pages.len() <= HELD_PAGES);
            if let Some(entries) = entries_read.as_mut() {
                entries.extend(bucket::entries(&page)?.into_iter().map(|e| (hash(&e.0), e)));
            }
            at = bucket::next(&page)?;
        }
        Ok((pages, entries_read))
    }

    /// Calls `each` of each entry of the chain whose pages are `pages`, in
    /// order, with its key's hash: of `held`, if given, as read before;
    /// else read a page at a time, a page's entries read before any of
    /// them is taken.
    ///
    /// # Errors
    ///
    /// The buffer pool's errors, or `each`'s.
    pub(super) fn each_entry(
        &self,
        (pages, held): (&[PageId], Option<Hashed>),
        each: &mut dyn FnMut(u64, Entry) -> Result<()>,
    ) -> Result<()> {
        if let Some(entries) = held {
            return entries.into_iter().try_for_each(|(h, e)| each(h, e));
        }
        for p in pages {
            for entry in bucket::entries(&self.pool.read(self.file, p.0)?)? {
                each(hash(&entry.0), entry)?;
            }
        }
        Ok(())
    }
}
