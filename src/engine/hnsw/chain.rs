//! \[DESIGN\] Byte streams in chains of pages, for the vector index
//! engines' images and change logs. Each page holds a tag, the format
//! version and the next page's id, then the stream's next bytes: as many
//! as its pager stores inline (`BufferPool::capacity`), so no page needs
//! overflow pages. A stream is read a page at a time, never whole.
//!
//! An appended stream (`Tail`) gains whole records only: the pages a
//! record needs are allocated before any is written, so a failed append
//! leaves the stream as it was. A record cut short at the end, as a
//! checkpoint taken during an append may hold, is cut off when the stream
//! is opened again (`Tail::cut`).

use std::collections::HashSet;

use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::pager::PageId;
use crate::error::{Code, Result, SqlError};

/// A chain page's tag, 'C'.
const TAG: u8 = 67;
/// The chain page format.
const VERSION: u8 = 1;
/// A chain page's header: tag, version, next page.
pub const HEADER: usize = 10;
/// Where the next page's id is, after the tag and version.
const NEXT_AT: usize = 2;
/// The id of no page: the last page's next.
pub const NO_PAGE: u64 = u64::MAX;
/// \[DESIGN\] The fewest stream bytes a page must hold, so a varint or a
/// fixed field of a record crosses at most one page boundary per page.
const MIN_PAYLOAD: usize = 64;

/// The stream bytes each page of `file` holds.
///
/// # Errors
///
/// SQLCODE -910 if pages are too small to hold a chain.
pub fn payload(pool: &BufferPool, file: FileId) -> Result<usize> {
    let payload = pool.capacity(file)?.saturating_sub(HEADER);
    if payload < MIN_PAYLOAD {
        return Err(SqlError::new(Code::Api, "[DESIGN] pages are too small for a vector index"));
    }
    Ok(payload)
}

/// A page's header bytes, linking to `next`.
fn header(next: u64) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(HEADER);
    bytes.push(TAG);
    bytes.push(VERSION);
    bytes.extend_from_slice(&next.to_le_bytes());
    bytes
}

/// A new page holding only a header with no next page.
fn new_page(pool: &BufferPool, file: FileId) -> Result<PageId> {
    let (id, mut page) = pool.new_page(file)?;
    *page = header(NO_PAGE);
    Ok(id)
}

/// Frees `pages`, all of them tried; the first error, if any.
///
/// # Errors
///
/// The buffer pool's.
pub fn free_pages(pool: &BufferPool, file: FileId, pages: &[PageId]) -> Result<()> {
    let mut first = Ok(());
    for id in pages {
        let freed = pool.free(file, *id);
        if first.is_ok() {
            first = freed;
        }
    }
    first
}

/// Writes a new stream, a page as it fills.
pub struct Writer<'p> {
    pool: &'p BufferPool,
    file: FileId,
    payload: usize,
    current: PageId,
    buffer: Vec<u8>,
    pages: Vec<PageId>,
}

impl<'p> Writer<'p> {
    /// A stream in new pages of `file`.
    ///
    /// # Errors
    ///
    /// The buffer pool's; pages too small.
    pub fn new(pool: &'p BufferPool, file: FileId) -> Result<Writer<'p>> {
        let payload = payload(pool, file)?;
        let current = new_page(pool, file)?;
        Ok(Writer { pool, file, payload, current, buffer: Vec::with_capacity(payload), pages: vec![current] })
    }

    /// Appends `bytes`.
    ///
    /// # Errors
    ///
    /// The buffer pool's; the pages written are in `pages` to free.
    pub fn put(&mut self, mut bytes: &[u8]) -> Result<()> {
        while !bytes.is_empty() {
            if self.buffer.len() >= self.payload {
                let next = new_page(self.pool, self.file)?;
                self.pages.push(next);
                self.flush(next.0)?;
                self.current = next;
            }
            let room = self.payload.saturating_sub(self.buffer.len()).min(bytes.len());
            let (now, later) = bytes.split_at_checked(room).unwrap_or((bytes, &[]));
            self.buffer.extend_from_slice(now);
            bytes = later;
        }
        Ok(())
    }

    fn flush(&mut self, next: u64) -> Result<()> {
        let mut bytes = header(next);
        bytes.append(&mut self.buffer);
        *self.pool.write(self.file, self.current)? = bytes;
        Ok(())
    }

    /// Writes the last page: the stream's first page and every page.
    ///
    /// # Errors
    ///
    /// The buffer pool's.
    pub fn finish(mut self) -> Result<(PageId, Vec<PageId>)> {
        self.flush(NO_PAGE)?;
        let head = self.pages.first().copied().unwrap_or(self.current);
        Ok((head, self.pages))
    }

    /// Frees every page written, for a stream given up.
    ///
    /// # Errors
    ///
    /// The buffer pool's.
    pub fn abandon(self) -> Result<()> {
        free_pages(self.pool, self.file, &self.pages)
    }
}

/// Reads a stream from its first page, a page at a time.
pub struct Reader<'p> {
    pool: &'p BufferPool,
    file: FileId,
    page: Vec<u8>,
    at: usize,
    id: PageId,
    next: u64,
    seen: HashSet<u64>,
    ended: bool,
    /// Every page read, in order.
    pub pages: Vec<PageId>,
}

impl<'p> Reader<'p> {
    /// The stream whose first page is `head`: a cycle of links is
    /// corruption, never a loop.
    ///
    /// # Errors
    ///
    /// The buffer pool's; SQLCODE -916 for a malformed page.
    pub fn new(pool: &'p BufferPool, file: FileId, head: PageId) -> Result<Reader<'p>> {
        let mut reader = Reader {
            pool,
            file,
            page: Vec::new(),
            at: HEADER,
            id: head,
            next: head.0,
            seen: HashSet::new(),
            ended: false,
            pages: Vec::new(),
        };
        reader.advance()?;
        Ok(reader)
    }

    /// Reads the next page; false at the stream's end.
    fn advance(&mut self) -> Result<bool> {
        if self.next == NO_PAGE {
            return Ok(false);
        }
        if !self.seen.insert(self.next) {
            return Err(corrupt("its pages link in a cycle"));
        }
        let page = self.pool.read(self.file, self.next)?;
        let (tag, version) = (page.first().copied(), page.get(1).copied());
        let next = page.get(NEXT_AT..HEADER).and_then(|b| <[u8; 8]>::try_from(b).ok()).map(u64::from_le_bytes);
        let (Some(TAG), Some(VERSION), Some(next)) = (tag, version, next) else {
            return Err(corrupt("a page's tag or format version is unsupported"));
        };
        self.page.clear();
        self.page.extend_from_slice(&page);
        drop(page);
        self.id = PageId(self.next);
        self.pages.push(self.id);
        self.next = next;
        self.at = HEADER;
        Ok(true)
    }

    /// Whether every byte has been read.
    ///
    /// # Errors
    ///
    /// As `new`.
    pub fn at_end(&mut self) -> Result<bool> {
        while self.at >= self.page.len() {
            if !self.advance()? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Reads to the end: every page of the stream.
    ///
    /// # Errors
    ///
    /// As `new`.
    pub fn pages(mut self) -> Result<Vec<PageId>> {
        while self.advance()? {}
        Ok(self.pages)
    }

    /// Whether a read failed for the stream's end, not a malformed page.
    pub fn ended(&self) -> bool {
        self.ended
    }

    /// Where the next byte is: its page and its place in the page.
    pub fn mark(&self) -> (PageId, usize) {
        (self.id, self.at)
    }

    /// Fills `out` with the next bytes.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if the stream ends first.
    pub fn bytes(&mut self, out: &mut [u8]) -> Result<()> {
        let mut filled = 0_usize;
        while filled < out.len() {
            if self.at_end()? {
                self.ended = true;
                return Err(corrupt("it ends within a record"));
            }
            let have = self.page.get(self.at..).unwrap_or_default();
            let want = out.get_mut(filled..).unwrap_or_default();
            let n = have.len().min(want.len());
            want.iter_mut().zip(have).for_each(|(o, b)| *o = *b);
            filled = filled.saturating_add(n);
            self.at = self.at.saturating_add(n);
        }
        Ok(())
    }

    /// The next byte.
    ///
    /// # Errors
    ///
    /// As `bytes`.
    pub fn u8(&mut self) -> Result<u8> {
        let mut b = [0_u8; 1];
        self.bytes(&mut b)?;
        let [byte] = b;
        Ok(byte)
    }

    /// The next eight bytes, little-endian.
    ///
    /// # Errors
    ///
    /// As `bytes`.
    pub fn u64(&mut self) -> Result<u64> {
        let mut b = [0_u8; 8];
        self.bytes(&mut b)?;
        Ok(u64::from_le_bytes(b))
    }

    /// The next LEB128 varint.
    ///
    /// # Errors
    ///
    /// As `bytes`; SQLCODE -916 past 64 bits.
    pub fn varint(&mut self) -> Result<u64> {
        let mut value = 0_u64;
        for shift in (0..u64::BITS).step_by(VARINT_BITS) {
            let byte = self.u8()?;
            value |= u64::from(byte & VARINT_MASK).checked_shl(shift).unwrap_or(0);
            if byte & VARINT_MORE == 0 {
                return Ok(value);
            }
        }
        Err(corrupt("a varint is longer than 64 bits"))
    }

    /// Fills `out` with the next little-endian floats.
    ///
    /// # Errors
    ///
    /// As `bytes`.
    pub fn floats(&mut self, out: &mut [f32]) -> Result<()> {
        let mut b = [0_u8; 4];
        for x in out {
            self.bytes(&mut b)?;
            *x = f32::from_bits(u32::from_le_bytes(b));
        }
        Ok(())
    }
}

/// The bits of a varint byte holding the value.
const VARINT_BITS: usize = 7;
/// The value's bits of a varint byte.
const VARINT_MASK: u8 = 0x7F;
/// The bit of a varint byte saying another follows.
const VARINT_MORE: u8 = 0x80;

/// Appends `value` as a LEB128 varint.
pub fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value > u64::from(VARINT_MASK) {
        out.push(u8::try_from(value & u64::from(VARINT_MASK)).unwrap_or(0) | VARINT_MORE);
        value >>= VARINT_BITS;
    }
    out.push(u8::try_from(value).unwrap_or(0));
}

/// Appends little-endian floats.
pub fn put_floats(out: &mut Vec<u8>, floats: &[f32]) {
    for x in floats {
        out.extend_from_slice(&x.to_bits().to_le_bytes());
    }
}

/// The end of an appended stream: its first page and its last.
#[derive(Debug, Clone, Copy)]
pub struct Tail {
    /// The stream's first page.
    pub head: PageId,
    /// Its last page, where records are appended.
    pub last: PageId,
}

impl Tail {
    /// A new empty stream.
    ///
    /// # Errors
    ///
    /// The buffer pool's.
    pub fn create(pool: &BufferPool, file: FileId) -> Result<Tail> {
        payload(pool, file)?;
        let head = new_page(pool, file)?;
        Ok(Tail { head, last: head })
    }

    /// Appends `record` whole, or, failing, nothing.
    ///
    /// # Errors
    ///
    /// The buffer pool's.
    pub fn append(&mut self, pool: &BufferPool, file: FileId, record: &[u8]) -> Result<()> {
        let payload = payload(pool, file)?;
        let used = pool.read(file, self.last.0)?.len().saturating_sub(HEADER);
        let room = payload.saturating_sub(used);
        let more = record.len().saturating_sub(room).div_ceil(payload);
        let mut fresh = Vec::with_capacity(more);
        for _ in 0..more {
            match new_page(pool, file) {
                Ok(id) => fresh.push(id),
                Err(error) => {
                    // Never linked, so nothing refers to them; a failed
                    // free leaks pages only.
                    let _ = free_pages(pool, file, &fresh);
                    return Err(error);
                }
            }
        }
        let (now, mut later) = record.split_at_checked(room).unwrap_or((record, &[]));
        let mut current = self.last;
        pool.write(file, current)?.extend_from_slice(now);
        for id in fresh {
            link(pool, file, current, id.0)?;
            let (part, rest) = later.split_at_checked(payload).unwrap_or((later, &[]));
            pool.write(file, id)?.extend_from_slice(part);
            later = rest;
            current = id;
        }
        self.last = current;
        Ok(())
    }

    /// The stream of `head`, read to its end; a record cut short at the
    /// end, from `mark`, the end of the last whole record, cut off.
    ///
    /// # Errors
    ///
    /// The buffer pool's.
    pub fn cut(pool: &BufferPool, file: FileId, head: PageId, (mark, pages): ((PageId, usize), &[PageId])) -> Result<Tail> {
        let (last, end) = mark;
        let after: Vec<PageId> = pages.iter().skip_while(|id| **id != last).skip(1).copied().collect();
        {
            let mut page = pool.write(file, last)?;
            page.truncate(end.max(HEADER));
        }
        link(pool, file, last, NO_PAGE)?;
        free_pages(pool, file, &after)?;
        Ok(Tail { head, last })
    }
}

/// Makes page `id`'s next page `next`.
fn link(pool: &BufferPool, file: FileId, id: PageId, next: u64) -> Result<()> {
    let mut page = pool.write(file, id)?;
    let field = page.get_mut(NEXT_AT..HEADER).ok_or_else(|| corrupt("a page has no header"))?;
    field.iter_mut().zip(next.to_le_bytes()).for_each(|(f, b)| *f = b);
    Ok(())
}

/// A malformed chain.
pub fn corrupt(why: &str) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[DESIGN] a vector index's pages are malformed: {why}"))
}
