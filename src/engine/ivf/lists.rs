//! \[DESIGN\] IVF posting lists and their directory in pages, format 1.
//! A list is a chain of pages, each a header (tag, version, next page)
//! then fixed-width entries: row, count, vector hash, the squared norm of
//! the vector a scan reads (its code's decoded, if coded), then the
//! vector's floats or its code. A metric measured by norms (COSINE) reads
//! the norm with the vector, and sums only `q·v` an entry. Entries are appended at the list's last page, and
//! a remove lowers a count in place, a count of 0 a tombstone, so a page
//! never moves an entry and a scan never misses one. A new last page is
//! written whole before the old one links to it.
//!
//! The directory holds each list's first and last pages and counts, a
//! fixed record each, so one list's change writes one directory page.

use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::pager::PageId;
use crate::engine::hnsw::{NO_PAGE, corrupt};
use crate::error::{Code, Result, SqlError};

/// A list page's tag, 'L'.
const LIST_TAG: u8 = 76;
/// A directory page's tag, 'D'.
const DIRECTORY_TAG: u8 = 68;
/// The page formats.
const VERSION: u8 = 1;
/// A page's header: tag, version, next page.
pub const HEADER: usize = 10;
/// Where the next page's id is.
const NEXT_AT: usize = 2;
/// An entry's bytes before its vector: row, count, hash, norm.
pub const ENTRY_HEAD: usize = 24;
/// Where an entry's count is.
const COUNT_AT: usize = 8;
/// Where an entry's hash is.
const HASH_AT: usize = 12;
/// Where an entry's squared norm is.
const NORM_AT: usize = 20;
/// A directory record's bytes: first page, last page, live, dead.
const RECORD: usize = 32;
/// The bytes of a count.
const COUNT_BYTES: usize = 4;
/// The bytes of each of a directory record's fields.
const FIELD_BYTES: usize = 8;

/// How entries lie in pages.
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    /// An entry's bytes.
    pub width: usize,
    /// Entries a page holds.
    pub per_page: usize,
    /// Directory records a page holds.
    pub per_directory: usize,
}

impl Layout {
    /// Entries of `payload` bytes in pages of `file`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if an entry does not fit a page, naming the bytes it needs.
    pub fn new(pool: &BufferPool, file: FileId, payload: usize) -> Result<Layout> {
        let room = pool.capacity(file)?.saturating_sub(HEADER);
        let width = ENTRY_HEAD.saturating_add(payload);
        let per_page = room.checked_div(width).unwrap_or(0);
        if per_page == 0 || room < RECORD {
            return Err(SqlError::new(
                Code::Api,
                format!(
                    "[DESIGN] an IVF entry needs {} bytes of a page, but pages hold {room}: use larger pages or a quantizer",
                    width.saturating_add(HEADER)
                ),
            ));
        }
        Ok(Layout { width, per_page, per_directory: room.checked_div(RECORD).unwrap_or(1) })
    }
}

/// One list: its first and last pages, its live entries and tombstones.
#[derive(Debug, Clone, Copy)]
pub struct List {
    /// Its first page.
    pub head: PageId,
    /// Its last page, where entries are appended.
    pub tail: PageId,
    /// Entries with a count.
    pub live: u64,
    /// Tombstones.
    pub dead: u64,
}

/// A page header of `tag` linking to `next`.
fn header(tag: u8, next: u64) -> Vec<u8> {
    let mut bytes = vec![tag, VERSION];
    bytes.extend_from_slice(&next.to_le_bytes());
    bytes
}

/// The next page `bytes`, a page of `tag`, links to.
fn next_of(bytes: &[u8], tag: u8) -> Result<u64> {
    let next = bytes.get(NEXT_AT..HEADER).and_then(|b| <[u8; 8]>::try_from(b).ok()).map(u64::from_le_bytes);
    let (Some(found), Some(version), Some(next)) = (bytes.first(), bytes.get(1), next) else {
        return Err(corrupt("an IVF page has no header"));
    };
    if *found != tag || *version != VERSION {
        return Err(corrupt("an IVF page's tag or format version is unsupported"));
    }
    Ok(next)
}

/// A new empty list.
///
/// # Errors
///
/// The buffer pool's.
pub fn new_list(pool: &BufferPool, file: FileId) -> Result<List> {
    let (id, mut page) = pool.new_page(file)?;
    *page = header(LIST_TAG, NO_PAGE);
    Ok(List { head: id, tail: id, live: 0, dead: 0 })
}

/// Appends `entry` to `list`: the page it went to.
///
/// # Errors
///
/// The buffer pool's; nothing is appended then.
pub fn append(pool: &BufferPool, file: FileId, (layout, list): (&Layout, &mut List), entry: &[u8]) -> Result<PageId> {
    {
        let mut page = pool.write(file, list.tail)?;
        let held = page.len().saturating_sub(HEADER).checked_div(layout.width).unwrap_or(0);
        if held < layout.per_page {
            page.extend_from_slice(entry);
            return Ok(list.tail);
        }
    }
    let id = {
        let (id, mut page) = pool.new_page(file)?;
        let mut bytes = header(LIST_TAG, NO_PAGE);
        bytes.extend_from_slice(entry);
        *page = bytes;
        id
    };
    if let Err(error) = link(pool, file, list.tail, id.0) {
        // Never linked: nothing refers to it, and a failed free leaks it only.
        let _ = pool.free(file, id);
        return Err(error);
    }
    list.tail = id;
    Ok(id)
}

/// Makes page `id`'s next page `next`.
fn link(pool: &BufferPool, file: FileId, id: PageId, next: u64) -> Result<()> {
    let mut page = pool.write(file, id)?;
    let field = page.get_mut(NEXT_AT..HEADER).ok_or_else(|| corrupt("an IVF page has no header"))?;
    field.iter_mut().zip(next.to_le_bytes()).for_each(|(f, b)| *f = b);
    Ok(())
}

/// An entry's bytes, appended to `out`: its vector stored as `payload`,
/// which reads back as a vector of squared norm `norm`.
pub fn entry(out: &mut Vec<u8>, (row, count, hash): (u64, u32, u64), (payload, norm): (&[u8], f32)) {
    out.extend_from_slice(&row.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&hash.to_le_bytes());
    out.extend_from_slice(&norm.to_bits().to_le_bytes());
    out.extend_from_slice(payload);
}

/// An entry's squared norm.
pub fn norm(entry: &[u8]) -> f32 {
    entry.get(NORM_AT..ENTRY_HEAD).and_then(|b| b.try_into().ok()).map_or(0.0, |b| f32::from_bits(u32::from_le_bytes(b)))
}

/// An entry's row, count and hash.
pub fn fields(entry: &[u8]) -> Option<(u64, u32, u64)> {
    let row = u64::from_le_bytes(entry.get(..COUNT_AT)?.try_into().ok()?);
    let count = u32::from_le_bytes(entry.get(COUNT_AT..HASH_AT)?.try_into().ok()?);
    let hash = u64::from_le_bytes(entry.get(HASH_AT..NORM_AT)?.try_into().ok()?);
    Some((row, count, hash))
}

/// Reads page `id` of a list into `buf`: the page it links to next, if any.
///
/// # Errors
///
/// The buffer pool's; SQLCODE -916 for a malformed page.
pub fn read_page(pool: &BufferPool, file: FileId, id: PageId, buf: &mut Vec<u8>) -> Result<Option<PageId>> {
    let page = pool.read(file, id.0)?;
    let next = next_of(&page, LIST_TAG)?;
    buf.clear();
    buf.extend_from_slice(page.get(HEADER..).unwrap_or_default());
    Ok((next != NO_PAGE).then_some(PageId(next)))
}

/// Changes the count of the entry at `slot` of page `id` by one, up if
/// `up`: the count it had.
///
/// # Errors
///
/// The buffer pool's; SQLCODE -916 if there is no such entry; -910 if
/// the count would pass its range.
pub fn count(pool: &BufferPool, file: FileId, (layout, id, slot): (&Layout, PageId, usize), up: bool) -> Result<u32> {
    let mut page = pool.write(file, id)?;
    let at = HEADER.saturating_add(slot.saturating_mul(layout.width)).saturating_add(COUNT_AT);
    let field = page.get_mut(at..at.saturating_add(COUNT_BYTES)).ok_or_else(|| corrupt("an IVF entry is missing"))?;
    let before = u32::from_le_bytes(<[u8; 4]>::try_from(&*field).map_err(|_| corrupt("an IVF entry is missing"))?);
    let after = if up { before.checked_add(1) } else { before.checked_sub(1) };
    let after = after.ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] an IVF entry's count would pass its range"))?;
    field.iter_mut().zip(after.to_le_bytes()).for_each(|(f, b)| *f = b);
    Ok(before)
}

/// What a walk gives each entry to: its page, slot and bytes.
pub type Each<'a> = dyn FnMut(PageId, usize, &[u8]) -> Result<()> + 'a;

/// Gives `each` every entry of the list whose first page is `head`, with
/// its page and slot, reading a page at a time into `buf`.
///
/// # Errors
///
/// As `read_page`; `each`'s.
pub fn walk(pool: &BufferPool, file: FileId, (head, width): (PageId, usize), buf: &mut Vec<u8>, each: &mut Each<'_>) -> Result<()> {
    let mut next = Some(head);
    while let Some(page) = next {
        next = read_page(pool, file, page, buf)?;
        for (slot, e) in buf.chunks_exact(width).enumerate() {
            each(page, slot, e)?;
        }
    }
    Ok(())
}

/// Every page of the list whose first page is `head`.
///
/// # Errors
///
/// As `read_page`; SQLCODE -916 for pages linked in a cycle.
pub fn pages_of(pool: &BufferPool, file: FileId, head: PageId) -> Result<Vec<PageId>> {
    let mut pages = vec![head];
    let mut seen = std::collections::HashSet::from([head.0]);
    let mut next = next_of(&pool.read(file, head.0)?, LIST_TAG)?;
    while next != NO_PAGE {
        if !seen.insert(next) {
            return Err(corrupt("an IVF list links in a cycle"));
        }
        pages.push(PageId(next));
        next = next_of(&pool.read(file, next)?, LIST_TAG)?;
    }
    Ok(pages)
}

/// Writes `lists`' directory to new pages: its pages, first first.
///
/// # Errors
///
/// The buffer pool's, of a page made or of one written; the pages made
/// are freed then (RULES.md D1044).
pub fn write_directory(pool: &BufferPool, file: FileId, layout: &Layout, lists: &[List]) -> Result<Vec<PageId>> {
    let chunks: Vec<&[List]> = lists.chunks(layout.per_directory.max(1)).collect();
    let mut pages = Vec::with_capacity(chunks.len().max(1));
    let written = made(pool, file, chunks.len().max(1), &mut pages).and_then(|()| filled(pool, file, &pages, &chunks));
    if let Err(error) = written {
        // Unpublished; a failed free leaks pages only.
        let _ = crate::engine::hnsw::free_pages(pool, file, &pages);
        return Err(error);
    }
    Ok(pages)
}

/// Makes `count` pages, each pushed to `pages` as it is made.
fn made(pool: &BufferPool, file: FileId, count: usize, pages: &mut Vec<PageId>) -> Result<()> {
    for _ in 0..count {
        pages.push(pool.new_page(file)?.0);
    }
    Ok(())
}

/// Writes each of `chunks`, a directory page's lists, to its page of
/// `pages`, linked to the next.
fn filled(pool: &BufferPool, file: FileId, pages: &[PageId], chunks: &[&[List]]) -> Result<()> {
    for (at, id) in pages.iter().enumerate() {
        let next = pages.get(at.saturating_add(1)).map_or(NO_PAGE, |p| p.0);
        let mut bytes = header(DIRECTORY_TAG, next);
        for list in chunks.get(at).copied().unwrap_or_default() {
            record(&mut bytes, list);
        }
        *pool.write(file, *id)? = bytes;
    }
    Ok(())
}

/// A list's directory record, appended to `out`.
fn record(out: &mut Vec<u8>, list: &List) {
    for field in [list.head.0, list.tail.0, list.live, list.dead] {
        out.extend_from_slice(&field.to_le_bytes());
    }
}

/// The directory of `count` lists whose first page is `head`, and its pages.
///
/// # Errors
///
/// The buffer pool's; SQLCODE -916 for a malformed directory.
pub fn read_directory(pool: &BufferPool, file: FileId, head: PageId, count: usize) -> Result<(Vec<List>, Vec<PageId>)> {
    let (mut lists, mut pages) = (Vec::with_capacity(count), Vec::new());
    let mut next = head.0;
    while next != NO_PAGE && pages.len() <= count {
        let page = pool.read(file, next)?;
        pages.push(PageId(next));
        next = next_of(&page, DIRECTORY_TAG)?;
        for r in page.get(HEADER..).unwrap_or_default().as_chunks::<RECORD>().0 {
            let mut fields = r.as_chunks::<FIELD_BYTES>().0.iter().map(|b| u64::from_le_bytes(*b));
            let mut field = || fields.next().unwrap_or(0);
            lists.push(List { head: PageId(field()), tail: PageId(field()), live: field(), dead: field() });
        }
    }
    if lists.len() != count || next != NO_PAGE {
        return Err(corrupt("an IVF directory holds the wrong count of lists"));
    }
    Ok((lists, pages))
}

/// Writes `lists`' records over the directory's pages, `directory`, in place.
///
/// # Errors
///
/// The buffer pool's; SQLCODE -916 if the pages are too few.
pub fn rewrite_directory(pool: &BufferPool, file: FileId, (layout, directory): (&Layout, &[PageId]), lists: &[List]) -> Result<()> {
    let chunks: Vec<&[List]> = lists.chunks(layout.per_directory.max(1)).collect();
    if chunks.len() > directory.len() {
        return Err(corrupt("an IVF directory has too few pages"));
    }
    for (at, id) in directory.iter().enumerate() {
        let next = directory.get(at.saturating_add(1)).map_or(NO_PAGE, |p| p.0);
        let mut bytes = header(DIRECTORY_TAG, next);
        for list in chunks.get(at).copied().unwrap_or_default() {
            record(&mut bytes, list);
        }
        *pool.write(file, *id)? = bytes;
    }
    Ok(())
}
