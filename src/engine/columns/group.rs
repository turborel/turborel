//! \[DESIGN\] A frozen row group of the column layout (RULES.md D31): rows
//! committed before every snapshot, in row id order, each column's values
//! in chunks (`columns.rs`) on pages of their own; for each chunk, the rows
//! it holds, its least and greatest values and its nulls, so a scan reads
//! only the chunks a condition may match (zone maps). A group is written
//! once and never changed: a change of one of its rows is the delta's.
//!
//! Its row ids are its first and last if every id between is its, else
//! chunks of their own, each id 8 bytes. What describes it, its header, is
//! kept on a chain of pages:
//!
//! ```text
//! page:    next page u64 (0 for none) | the header's bytes
//! header:  GROUP_FORMAT u8 | first id u64 | last id u64 | rows u32 |
//!          id runs u32 | each: run | columns u32 | each: runs u32 | each:
//!          run | nulls u32 | bounds u8 (1 if any) | least | greatest
//! run:     first row u32 | rows u32 | page u64
//! bound:   length u32 | the value as its type writes it in a record
//! ```
//!
//! Integers are little-endian. Every page passes through the table's
//! encodings, its page number their context.

mod header;

use std::borrow::Cow;
use std::cmp::Ordering;

pub use header::read;

use super::{Chunk, Fields};
use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::pager::PageId;
use crate::error::{Code, Result, SqlError};
use crate::storage::{Row, RowId};
use crate::value::{DataType, Value, compare};

/// The format of a group's header.
const GROUP_FORMAT: u8 = 1;
/// Bytes of a row id.
const ID: usize = 8;
/// Bytes of a chain page's link.
const LINK: usize = 8;

fn corrupt(why: &str) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[DESIGN] a frozen row group is malformed: {why}"))
}

fn too_large() -> SqlError {
    SqlError::new(Code::Unsupported, "[DESIGN] a frozen row group would be too large")
}

/// How a group's pages are sealed as they are stored, and opened as they
/// are read: by the table's encodings, the page's number the context.
pub trait Seal {
    /// `content`, as page `page` stores it.
    ///
    /// # Errors
    ///
    /// An encoding's.
    fn seal(&self, page: u64, content: &[u8]) -> Result<Vec<u8>>;

    /// The content page `page` stores as `stored`: `stored` itself, if
    /// it passed through no encoding.
    ///
    /// # Errors
    ///
    /// An encoding's: SQLCODE -916 if `stored` is not what it sealed.
    fn open<'a>(&self, page: u64, stored: &'a [u8]) -> Result<Cow<'a, [u8]>>;
}

/// Where a group's pages are, and how each is sealed.
pub struct Pages<'a, S> {
    /// The pool the pages are read and written through.
    pub pool: &'a BufferPool,
    /// The pager file they are in.
    pub file: FileId,
    /// The content a page holds, less what sealing adds.
    pub capacity: usize,
    /// How its pages are sealed.
    pub sealing: &'a S,
}

impl<S: Seal> Pages<'_, S> {
    /// A new page holding `content`, sealed.
    fn put(&self, content: &[u8], written: &mut Vec<PageId>) -> Result<PageId> {
        let (page, mut held) = self.pool.new_page(self.file)?;
        written.push(page);
        *held = self.sealing.seal(page.0, content)?;
        Ok(page)
    }

    /// The content of page `page`, opened.
    ///
    /// # Errors
    ///
    /// The pool's; the encodings'.
    pub fn get(&self, page: PageId) -> Result<Vec<u8>> {
        self.with(page, |content| Ok(content.to_vec()))
    }

    /// What `read` makes of the content of page `page`, opened, read in
    /// the pool where it passed through no encoding.
    ///
    /// # Errors
    ///
    /// The pool's; the encodings'; `read`'s.
    pub fn with<T>(&self, page: PageId, read: impl FnOnce(&[u8]) -> Result<T>) -> Result<T> {
        let stored = self.pool.read(self.file, page.0)?;
        read(&self.sealing.open(page.0, &stored)?)
    }

    /// Frees `pages`; one that cannot be is left.
    pub fn free(&self, pages: &[PageId]) {
        for &page in pages {
            let _ = self.pool.free(self.file, page);
        }
    }
}

/// A run of a group's rows, on one page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Run {
    /// The first row's position in the group.
    pub first: u32,
    /// How many rows.
    pub rows: u32,
    /// The page they are on.
    pub page: PageId,
}

/// A chunk of a column, and what a scan may skip it by.
#[derive(Debug, Clone)]
pub struct Zone {
    /// The rows it holds, and its page.
    pub run: Run,
    /// How many of its values are null.
    pub nulls: u32,
    /// Its least and greatest values that are not null, if any.
    pub bounds: Option<(Value, Value)>,
}

/// \[DESIGN\] A frozen row group: its row ids, and its columns' chunks.
#[derive(Debug, Clone)]
pub struct Group {
    /// Its first row id.
    pub first: RowId,
    /// Its last row id.
    pub last: RowId,
    /// How many rows it holds.
    pub rows: u32,
    /// The chunks of its row ids; none if they are every id from first to
    /// last.
    pub ids: Vec<Run>,
    /// Each column's chunks, in order.
    pub columns: Vec<Vec<Zone>>,
    /// The pages of its header.
    pub header: Vec<PageId>,
}

/// The least and greatest of `values` that are not null, and the nulls.
fn bounds_of(values: &[Value]) -> (Option<(Value, Value)>, u32) {
    let mut bounds: Option<(Value, Value)> = None;
    let mut nulls = 0_u32;
    for v in values {
        if v.is_null() {
            nulls = nulls.saturating_add(1);
            continue;
        }
        bounds = Some(match bounds {
            None => (v.clone(), v.clone()),
            Some((least, greatest)) => {
                let least = if compare(v, &least) == Some(Ordering::Less) { v.clone() } else { least };
                let greatest = if compare(v, &greatest) == Some(Ordering::Greater) { v.clone() } else { greatest };
                (least, greatest)
            }
        });
    }
    (bounds, nulls)
}

fn count32(n: usize) -> Result<u32> {
    u32::try_from(n).map_err(|_| too_large())
}

/// Writes the column `column` of `rows`, of type `t`, in chunks.
fn write_column<S: Seal>(
    pages: &Pages<'_, S>,
    t: &DataType,
    (rows, column): (&[Row], usize),
    written: &mut Vec<PageId>,
) -> Result<Vec<Zone>> {
    let values: Vec<Value> = rows.iter().map(|r| r.get(column).cloned().unwrap_or(Value::Null)).collect();
    let fields = Fields::of(t, values.iter().cloned())?;
    let mut zones = Vec::new();
    let mut from = 0_usize;
    while from < fields.len() {
        let count = fields.fitting(t, from, pages.capacity);
        let page = pages.put(&fields.chunk(t, from, count)?, written)?;
        let (bounds, nulls) = bounds_of(values.get(from..from.saturating_add(count)).unwrap_or_default());
        zones.push(Zone { run: Run { first: count32(from)?, rows: count32(count)?, page }, nulls, bounds });
        from = from.saturating_add(count);
    }
    Ok(zones)
}

/// Writes `ids`, if they are not every id from their first to their last.
fn write_ids<S: Seal>(pages: &Pages<'_, S>, ids: &[RowId], written: &mut Vec<PageId>) -> Result<Vec<Run>> {
    let (Some(&first), Some(&last)) = (ids.first(), ids.last()) else { return Ok(Vec::new()) };
    if last.checked_sub(first).and_then(|span| span.checked_add(1)) == u64::try_from(ids.len()).ok() {
        return Ok(Vec::new());
    }
    let per_page = pages.capacity.checked_div(ID).unwrap_or(1).max(1);
    let mut runs = Vec::new();
    for (n, chunk) in ids.chunks(per_page).enumerate() {
        let bytes: Vec<u8> = chunk.iter().flat_map(|id| id.to_le_bytes()).collect();
        let page = pages.put(&bytes, written)?;
        runs.push(Run { first: count32(n.saturating_mul(per_page))?, rows: count32(chunk.len())?, page });
    }
    Ok(runs)
}

impl Group {
    /// Writes `rows`, whose ids are `ids`, in increasing order, a column
    /// of each of `types`, as a group.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the rows are not of the types; -900 if there are too
    /// many; an error of the pages; nothing written is kept.
    pub fn write<S: Seal>(pages: &Pages<'_, S>, types: &[DataType], ids: &[RowId], rows: &[Row]) -> Result<Group> {
        let mut written = Vec::new();
        let made = Self::write_all(pages, types, (ids, rows), &mut written);
        if made.is_err() {
            pages.free(&written);
        }
        made
    }

    fn write_all<S: Seal>(
        pages: &Pages<'_, S>,
        types: &[DataType],
        (ids, rows): (&[RowId], &[Row]),
        written: &mut Vec<PageId>,
    ) -> Result<Group> {
        let (Some(&first), Some(&last)) = (ids.first(), ids.last()) else {
            return Err(SqlError::new(Code::Api, "[DESIGN] a frozen row group of no rows"));
        };
        let id_runs = write_ids(pages, ids, written)?;
        let columns = types.iter().enumerate().map(|(c, t)| write_column(pages, t, (rows, c), written)).collect::<Result<Vec<_>>>()?;
        let mut group = Group { first, last, rows: count32(rows.len())?, ids: id_runs, columns, header: Vec::new() };
        group.header = put_chain(pages, &header::encode(&group, types)?, written)?;
        Ok(group)
    }

    /// Every page it is on.
    pub fn pages(&self) -> Vec<PageId> {
        let chunks = self.columns.iter().flatten().map(|z| z.run.page);
        self.ids.iter().map(|r| r.page).chain(chunks).chain(self.header.iter().copied()).collect()
    }

    /// Its row ids, in order.
    ///
    /// # Errors
    ///
    /// As `Pages::get`; SQLCODE -916 if a page of them is malformed.
    pub fn row_ids<S: Seal>(&self, pages: &Pages<'_, S>) -> Result<Vec<RowId>> {
        if self.ids.is_empty() {
            return Ok((self.first..=self.last).collect());
        }
        let mut out = Vec::with_capacity(usize::try_from(self.rows).unwrap_or(0));
        for run in &self.ids {
            out.extend(pages.with(run.page, ids_of)?);
        }
        Ok(out)
    }

    /// The position of row `id` in the group, if it is there.
    ///
    /// # Errors
    ///
    /// As `row_ids`.
    pub fn position<S: Seal>(&self, pages: &Pages<'_, S>, id: RowId) -> Result<Option<u32>> {
        if id < self.first || id > self.last {
            return Ok(None);
        }
        if self.ids.is_empty() {
            return Ok(u32::try_from(id.saturating_sub(self.first)).ok());
        }
        // The last run whose first id is at most `id`.
        let mut found = None;
        for run in &self.ids {
            let ids = pages.with(run.page, ids_of)?;
            if ids.first().is_some_and(|&f| f > id) {
                break;
            }
            found = ids.binary_search(&id).ok().map(|at| run.first.saturating_add(u32::try_from(at).unwrap_or(u32::MAX)));
        }
        Ok(found)
    }

    /// The value of column `column`, of type `t`, of the row at `position`.
    ///
    /// # Errors
    ///
    /// As `Pages::get`; SQLCODE -916 if its chunk is malformed.
    pub fn value<S: Seal>(&self, pages: &Pages<'_, S>, (column, t): (usize, &DataType), position: u32) -> Result<Value> {
        let zones = self.columns.get(column).ok_or_else(|| corrupt("a column is missing"))?;
        let zone = zones
            .iter()
            .find(|z| position >= z.run.first && position < z.run.first.saturating_add(z.run.rows))
            .ok_or_else(|| corrupt("a row is in no chunk"))?;
        let at = usize::try_from(position.saturating_sub(zone.run.first)).unwrap_or(usize::MAX);
        pages.with(zone.run.page, |bytes| Chunk::parse(bytes)?.value(at, t))
    }
}

/// The ids a page of them holds.
fn ids_of(bytes: &[u8]) -> Result<Vec<RowId>> {
    let (ids, rest) = bytes.as_chunks::<ID>();
    if !rest.is_empty() {
        return Err(corrupt("a page of row ids is not whole"));
    }
    Ok(ids.iter().map(|b| u64::from_le_bytes(*b)).collect())
}

/// Writes `bytes` on a chain of pages; its pages, the first first. On
/// failure, none is left allocated.
///
/// # Errors
///
/// The pages'.
pub fn write_chain<S: Seal>(pages: &Pages<'_, S>, bytes: &[u8]) -> Result<Vec<PageId>> {
    let mut written = Vec::new();
    let chain = put_chain(pages, bytes, &mut written);
    if chain.is_err() {
        pages.free(&written);
    }
    chain
}

/// Writes `bytes` on a chain of pages, each page written added to
/// `written`; its pages, the first first.
fn put_chain<S: Seal>(pages: &Pages<'_, S>, bytes: &[u8], written: &mut Vec<PageId>) -> Result<Vec<PageId>> {
    let per_page = pages.capacity.saturating_sub(LINK).max(1);
    let parts: Vec<&[u8]> = bytes.chunks(per_page).collect();
    // Written last first, so each names the one after it.
    let mut chain = Vec::with_capacity(parts.len());
    let mut next = 0_u64;
    for part in parts.iter().rev() {
        let mut content = next.to_le_bytes().to_vec();
        content.extend_from_slice(part);
        let page = pages.put(&content, written)?;
        next = page.0;
        chain.push(page);
    }
    chain.reverse();
    Ok(chain)
}

/// The bytes of the chain whose first page is `first`, and its pages.
///
/// # Errors
///
/// As `Pages::get`; SQLCODE -916 if a page of it is malformed, or it
/// loops.
pub fn read_chain<S: Seal>(pages: &Pages<'_, S>, first: PageId) -> Result<(Vec<u8>, Vec<PageId>)> {
    let (mut bytes, mut chain) = (Vec::new(), Vec::new());
    let mut at = first.0;
    while at != 0 {
        if chain.contains(&PageId(at)) {
            return Err(corrupt("its header's pages loop"));
        }
        chain.push(PageId(at));
        let content = pages.get(PageId(at))?;
        let (link, rest) = content.split_first_chunk::<LINK>().ok_or_else(|| corrupt("a page of its header is too short"))?;
        bytes.extend_from_slice(rest);
        at = u64::from_le_bytes(*link);
    }
    Ok((bytes, chain))
}

// Unit tests: a group written reads back, by position and by id, dense
// and not, with nulls and long values; its header after a reopen; and
// malformed headers refused (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
