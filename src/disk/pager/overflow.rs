//! \[DESIGN\] Overflow (RULES.md D30): the content of a page that does not
//! fit in it. The page keeps as much as fits; the rest goes to overflow
//! pages, allocated in extents (runs of consecutive pages, from `free.rs`),
//! each written and read with one I/O. The page lists the first
//! `INLINE_EXTENTS` extents itself, so content allocated in a few runs needs
//! no other page; directory pages, chained, list the rest.

use super::format::{self, DATA_HEADER, ENTRY, INLINE_EXTENTS, KIND_DIRECTORY, KIND_OVERFLOW, LIST_HEADER, NONE, OVERFLOW_HEADER};
use super::{Extent, Pager, add};
use crate::error::Result;

impl Pager {
    /// Content bytes an overflow page holds.
    fn overflow_capacity(&self) -> usize {
        self.body_size().saturating_sub(OVERFLOW_HEADER).max(1)
    }

    /// Extents a directory page lists.
    fn directory_capacity(&self) -> usize {
        self.body_size().saturating_sub(LIST_HEADER).checked_div(ENTRY).unwrap_or(1).max(1)
    }

    /// Content bytes a data page holds with `extents` listed in it.
    fn inline_with(&self, extents: usize) -> usize {
        self.body_size().saturating_sub(DATA_HEADER).saturating_sub(extents.min(INLINE_EXTENTS).saturating_mul(ENTRY))
    }

    /// The body of a data page holding `content`, after writing what does
    /// not fit to new overflow pages; and every page allocated for it, to
    /// release if the data page cannot be written.
    pub(super) fn store(&self, content: &[u8]) -> Result<(Vec<u8>, Vec<Extent>)> {
        let length = u64::try_from(content.len()).unwrap_or(u64::MAX);
        if content.len() <= self.inline_with(0) {
            return Ok((format::data_body(length, &[], NONE, content), Vec::new()));
        }
        let extents = self.allocate_overflow(content.len());
        let inline = self.inline_with(extents.len()).min(content.len());
        let directories = self
            .allocate(u64::try_from(extents.len().saturating_sub(INLINE_EXTENTS).div_ceil(self.directory_capacity())).unwrap_or(u64::MAX));
        let mut allocated = extents.clone();
        allocated.extend_from_slice(&directories);
        let written = self
            .write_overflow(content.get(inline..).unwrap_or_default(), &extents)
            .and_then(|()| self.write_directories(&extents, &directories));
        if let Err(e) = written {
            // Nothing refers to these pages yet; if they cannot be released
            // they stay in use: lost space, not damage.
            let _ = self.release(&allocated);
            return Err(e);
        }
        let first_directory = directories.first().map_or(NONE, |d| d.start.0);
        let listed = extents.get(..INLINE_EXTENTS.min(extents.len())).unwrap_or_default();
        Ok((format::data_body(length, listed, first_directory, content.get(..inline).unwrap_or_default()), allocated))
    }

    /// Allocates the overflow pages for `length` content bytes: more are
    /// taken while listing the extents in the data page leaves too little.
    fn allocate_overflow(&self, length: usize) -> Vec<Extent> {
        let capacity = self.overflow_capacity();
        let mut extents: Vec<Extent> = Vec::new();
        let mut pages: usize = 0;
        loop {
            let needed = length.saturating_sub(self.inline_with(extents.len().max(1)));
            let more = needed.div_ceil(capacity).saturating_sub(pages);
            if more == 0 {
                return extents;
            }
            extents.extend(self.allocate(u64::try_from(more).unwrap_or(u64::MAX)));
            pages = pages.saturating_add(more);
        }
    }

    /// Writes `rest` to the pages of `extents`, one write per extent.
    fn write_overflow(&self, rest: &[u8], extents: &[Extent]) -> Result<()> {
        let mut chunks = rest.chunks(self.overflow_capacity());
        for e in extents {
            let mut sealed = Vec::new();
            for id in e.start.0..e.start.0.saturating_add(e.count) {
                self.seal(id, &format::overflow_body(chunks.next().unwrap_or_default()), &mut sealed);
            }
            self.write_run(e.start.0, &sealed)?;
            add(&self.counters.overflow_pages_written, e.count);
        }
        Ok(())
    }

    /// Writes the directory pages `directories`, chained, listing the
    /// extents after the first `INLINE_EXTENTS`.
    fn write_directories(&self, extents: &[Extent], directories: &[Extent]) -> Result<()> {
        let pages: Vec<u64> = directories.iter().flat_map(|d| d.start.0..d.start.0.saturating_add(d.count)).collect();
        let rest = extents.get(INLINE_EXTENTS..).unwrap_or_default();
        for (n, (&id, chunk)) in pages.iter().zip(rest.chunks(self.directory_capacity())).enumerate() {
            let next = pages.get(n.saturating_add(1)).copied().unwrap_or(NONE);
            let mut sealed = Vec::with_capacity(self.page_size);
            self.seal(id, &format::list_body(KIND_DIRECTORY, chunk, next), &mut sealed);
            self.write_run(id, &sealed)?;
            add(&self.counters.overflow_pages_written, 1);
        }
        Ok(())
    }

    /// The overflow extents of a data page's `body`, those it lists and
    /// those its directories list, and the directory pages themselves.
    fn extents_of(&self, body: &[u8]) -> Result<(Vec<Extent>, Vec<Extent>)> {
        let (_, mut extents, mut next, _) = format::data_header(body).ok_or_else(|| self.damaged("a data page is malformed"))?;
        let mut directories = Vec::new();
        while next != NONE {
            if u64::try_from(directories.len()).unwrap_or(u64::MAX) >= self.page_count() {
                return Err(self.damaged("an overflow directory loops"));
            }
            let dir = self
                .read_body(next)?
                .filter(|b| b.first() == Some(&KIND_DIRECTORY))
                .ok_or_else(|| self.damaged("a directory page is not one"))?;
            let (entries, after) = format::list_entries(&dir).ok_or_else(|| self.damaged("a directory page is malformed"))?;
            directories.push(Extent { start: super::PageId(next), count: 1 });
            extents.extend(entries);
            next = after;
        }
        Ok((extents, directories))
    }

    /// Every overflow and directory page of a data page's `body`.
    pub(super) fn overflow_pages(&self, body: &[u8]) -> Result<Vec<Extent>> {
        if body.first() != Some(&format::KIND_DATA) {
            return Ok(Vec::new());
        }
        let (mut extents, directories) = self.extents_of(body)?;
        extents.extend(directories);
        Ok(extents)
    }

    /// Reads the content of a data page's `body` into `content`: what the
    /// page holds, then its overflow pages, one read per extent.
    pub(super) fn load(&self, body: &[u8], content: &mut Vec<u8>) -> Result<()> {
        let (length, listed, _, start) = format::data_header(body).ok_or_else(|| self.damaged("a data page is malformed"))?;
        let length = usize::try_from(length).map_err(|_| self.damaged("a page's content is longer than memory"))?;
        let inline = body.get(start..).unwrap_or_default();
        content.extend_from_slice(inline.get(..length.min(inline.len())).unwrap_or_default());
        if content.len() == length && listed.is_empty() {
            return Ok(());
        }
        add(&self.counters.overflow_reads, 1);
        let (extents, _) = self.extents_of(body)?;
        for e in &extents {
            let pages = self.read_run(e.start.0, e.count)?;
            for (id, page) in (e.start.0..).zip(pages.chunks(self.page_size)) {
                let overflow = self
                    .unseal(id, page)?
                    .filter(|b| b.first() == Some(&KIND_OVERFLOW))
                    .ok_or_else(|| self.damaged("an overflow page is not one"))?;
                let chunk = overflow.get(OVERFLOW_HEADER..).unwrap_or_default();
                let wanted = length.saturating_sub(content.len()).min(chunk.len());
                content.extend_from_slice(chunk.get(..wanted).unwrap_or_default());
            }
        }
        if content.len() != length {
            return Err(self.damaged(&format!("a page's content is {} bytes, not the {length} it records", content.len())));
        }
        Ok(())
    }
}
