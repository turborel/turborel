//! \[DESIGN\] Compressed codes held in memory: "we store compressed vectors
//! x̃_p for every database point p ∈ P in main memory, along with storing
//! the graph on the SSD" [DiskANN, NeurIPS 2019 §3.1]. They are also kept
//! in code pages, a chunk directory of their own, so an index reopens
//! without re-encoding, and the codebook in a chain of pages. Each code
//! page holds room for as many codes as a page takes, the last one's room
//! past its codes zeros.
//!
//! The memory is reserved of the database's budget (D38). A codebook is
//! trained when a build loads the index, or by maintenance once an index
//! made by inserts has `Tuning::train_at` nodes; until then searches are
//! guided by full vectors.

use std::sync::Arc;

use crate::bytes::{Put, Reader};
use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::pager::PageId;
use crate::error::{Code, Result, SqlError};
use crate::memory::Reservation;
use crate::sync::{read, write};

use super::chunks::{Chunks, NO_PAGE};
use super::meta::Meta;
use super::node::{Node, corrupt};
use super::{Codes, DiskAnn};

/// Next page and length before a codebook page's bytes.
const CHAIN_HEADER: usize = 12;
/// What the codes' memory is reserved as.
const RESERVED_AS: &str = "the compressed codes of a DiskANN index";

/// The codebook and every node's code.
pub(super) struct Book {
    pub(super) codes: Option<Arc<dyn Codes>>,
    /// Bytes of a code, 0 until trained.
    pub(super) width: usize,
    /// Node i's code at i × width.
    pub(super) bytes: Vec<u8>,
    pub(super) pages: Chunks,
    /// The codebook's pages.
    pub(super) chain: Vec<PageId>,
    reservation: Option<Reservation>,
}

impl Book {
    pub(super) fn empty(pages: Chunks) -> Book {
        Book { codes: None, width: 0, bytes: Vec::new(), pages, chain: Vec::new(), reservation: None }
    }

    pub(super) fn free(&self, pool: &BufferPool, file: FileId) -> Result<()> {
        self.pages.free(pool, file)?;
        self.chain.iter().try_for_each(|&p| pool.free(file, p))
    }

    /// The code of node `id`, if it has one.
    pub(super) fn code(&self, id: u32) -> Option<&[u8]> {
        let at = usize::try_from(id).ok()?.checked_mul(self.width)?;
        self.bytes.get(at..at.checked_add(self.width)?)
    }
}

impl DiskAnn {
    /// Codes per code page.
    fn codes_per_page(&self, width: usize) -> Result<usize> {
        let per = self.inner.pool.capacity(self.inner.file)?.checked_div(width).unwrap_or(0);
        if per == 0 {
            return Err(SqlError::new(Code::Unsupported, "[EXTENSION] a DiskANN code is wider than a page"));
        }
        Ok(per)
    }

    /// Reads the codebook and codes `state` names, if any, into memory.
    pub(super) fn open_codes(&self, state: &Meta) -> Result<()> {
        let Some(quantizer) = self.inner.plugins.quantizer.clone() else { return Ok(()) };
        if state.book == NO_PAGE {
            return Ok(());
        }
        let (chain, bytes) = self.read_chain(PageId(state.book))?;
        let codes = quantizer.read(&bytes)?;
        let width = codes.code_bytes();
        if u32::try_from(width).ok() != Some(state.code_width) {
            return Err(corrupt("its codebook's code width differs from its codes'"));
        }
        let per = self.codes_per_page(width)?;
        let total = usize::try_from(state.next).unwrap_or(0).saturating_mul(width);
        let reservation = self.inner.budget.reserve(total, RESERVED_AS)?;
        let mut book = write(&self.inner.book)?;
        let mut all = Vec::with_capacity(total);
        for &page in &book.pages.pages {
            let content = self.inner.pool.read(self.inner.file, page)?;
            all.extend_from_slice(content.get(..per.saturating_mul(width)).ok_or_else(|| corrupt("a code page is short"))?);
        }
        all.truncate(total);
        all.resize(total, 0);
        *book =
            Book { codes: Some(codes), width, bytes: all, pages: std::mem::take(&mut book.pages), chain, reservation: Some(reservation) };
        Ok(())
    }

    fn read_chain(&self, head: PageId) -> Result<(Vec<PageId>, Vec<u8>)> {
        let (mut chain, mut bytes, mut next) = (Vec::new(), Vec::new(), head.0);
        while next != NO_PAGE {
            if u64::try_from(chain.len()).unwrap_or(u64::MAX) > self.inner.pool.pages_in_use(self.inner.file)? {
                return Err(corrupt("its codebook chain is cyclic"));
            }
            chain.push(PageId(next));
            let page = self.inner.pool.read(self.inner.file, next)?;
            let mut r = Reader::new(&page, "DiskANN codebook");
            next = r.u64()?;
            let length = usize::try_from(r.u32()?).unwrap_or(usize::MAX);
            bytes.extend_from_slice(r.take(length)?);
        }
        Ok((chain, bytes))
    }

    fn write_chain(&self, bytes: &[u8]) -> Result<Vec<PageId>> {
        let room = self.inner.pool.capacity(self.inner.file)?.saturating_sub(CHAIN_HEADER).max(1);
        let pieces: Vec<&[u8]> = bytes.chunks(room).collect();
        let mut pages = Vec::with_capacity(pieces.len());
        for _ in &pieces {
            let (id, page) = self.inner.pool.new_page(self.inner.file)?;
            drop(page);
            pages.push(id);
        }
        for (at, piece) in pieces.iter().enumerate() {
            let next = pages.get(at.saturating_add(1)).map_or(NO_PAGE, |p| p.0);
            let mut content = Vec::with_capacity(piece.len().saturating_add(CHAIN_HEADER));
            content.put_u64(next);
            content.put_u32(u32::try_from(piece.len()).unwrap_or(u32::MAX));
            content.extend_from_slice(piece);
            let page = pages.get(at).copied().ok_or_else(|| corrupt("a codebook page is missing"))?;
            *self.inner.pool.write(self.inner.file, page)? = content;
        }
        Ok(pages)
    }

    /// Trains a codebook on `sample` and encodes every node with it, at a
    /// build or by maintenance. Without a quantizer, does nothing.
    ///
    /// # Errors
    ///
    /// The quantizer's; SQLCODE -918 if the codes cannot be reserved; the
    /// buffer pool's. Searches go on guided by full vectors meanwhile.
    pub(super) fn train(&self, sample: &[f32]) -> Result<()> {
        let Some(quantizer) = self.inner.plugins.quantizer.clone() else { return Ok(()) };
        if sample.is_empty() {
            return Ok(());
        }
        let codes = quantizer.train(sample, self.inner.shape.dims, self.inner.seed)?;
        let width = codes.code_bytes();
        let per = self.codes_per_page(width)?;
        let next = lock_next(self);
        let total = usize::try_from(next).unwrap_or(0).saturating_mul(width);
        let reservation = self.inner.budget.reserve(total, RESERVED_AS)?;
        let mut all = vec![0_u8; total];
        let mut node = Node::default();
        for (id, code) in (0..next).zip(all.chunks_exact_mut(width.max(1))) {
            self.read_node(id, &mut node)?;
            codes.encode(&node.vector, code);
        }
        let chain = self.write_chain(&codes.write())?;
        let mut book = write(&self.inner.book)?;
        for (chunk, piece) in all.chunks(per.saturating_mul(width)).enumerate() {
            if chunk >= book.pages.pages.len() {
                let (page, content) = self.inner.pool.new_page(self.inner.file)?;
                drop(content);
                book.pages.push(&self.inner.pool, self.inner.file, page.0)?;
            }
            // Every code page holds room for `per` codes, as `set_code`
            // keeps it and `open_codes` reads it: the last one's room past
            // its codes is zeros.
            let mut content = piece.to_vec();
            content.resize(per.saturating_mul(width), 0);
            *self.inner.pool.write(self.inner.file, PageId(book.pages.page(chunk)?))? = content;
        }
        let old = std::mem::take(&mut book.chain);
        old.iter().try_for_each(|&p| self.inner.pool.free(self.inner.file, p))?;
        book.codes = Some(codes);
        book.width = width;
        book.bytes = all;
        book.chain = chain;
        book.reservation = Some(reservation);
        drop(book);
        // Nodes inserted while training found no codebook to encode by.
        for id in next..lock_next(self) {
            self.read_node(id, &mut node)?;
            self.set_code(id, &node.vector)?;
        }
        self.flush()
    }

    /// Encodes `vector` as node `id`'s code, in memory and in its page,
    /// if the index has codes.
    ///
    /// # Errors
    ///
    /// SQLCODE -918 if more memory cannot be reserved; the buffer pool's.
    pub(super) fn set_code(&self, id: u32, vector: &[f32]) -> Result<()> {
        let Some(codes) = read(&self.inner.book)?.codes.clone() else { return Ok(()) };
        let width = codes.code_bytes();
        let mut code = vec![0_u8; width];
        codes.encode(vector, &mut code);
        let per = self.codes_per_page(width)?;
        let at = usize::try_from(id).unwrap_or(usize::MAX);
        let (chunk, slot) = (at.checked_div(per).unwrap_or(0), at.checked_rem(per).unwrap_or(0));
        let mut book = write(&self.inner.book)?;
        if book.width != width {
            return Ok(());
        }
        let end = at.saturating_add(1).saturating_mul(width);
        if book.bytes.len() < end {
            grow(&mut book, end)?;
        }
        if let Some(place) = book.bytes.get_mut(end.saturating_sub(width)..end) {
            place.copy_from_slice(&code);
        }
        while book.pages.pages.len() <= chunk {
            let (page, content) = self.inner.pool.new_page(self.inner.file)?;
            drop(content);
            book.pages.push(&self.inner.pool, self.inner.file, page.0)?;
        }
        let mut page = self.inner.pool.write(self.inner.file, PageId(book.pages.page(chunk)?))?;
        let offset = slot.saturating_mul(width);
        if page.len() < offset.saturating_add(width) {
            page.resize(per.saturating_mul(width), 0);
        }
        if let Some(place) = page.get_mut(offset..offset.saturating_add(width)) {
            place.copy_from_slice(&code);
        }
        Ok(())
    }
}

/// Grows the codes in memory to `end` bytes, reserving a quarter more at
/// once so inserts reserve rarely.
fn grow(book: &mut Book, end: usize) -> Result<()> {
    /// The share more reserved: a quarter.
    const SLACK: usize = 4;
    if let Some(reservation) = book.reservation.as_mut().filter(|r| r.bytes() < end) {
        reservation.resize(end.saturating_add(end.checked_div(SLACK).unwrap_or(0)))?;
    }
    book.bytes.resize(end, 0);
    Ok(())
}

fn lock_next(index: &DiskAnn) -> u32 {
    crate::sync::lock_builtin(&index.inner.alloc).next
}
