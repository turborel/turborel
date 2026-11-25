//! \[DESIGN\] A frozen row group of the column layout (RULES.md D31): rows
//! committed before every snapshot, in row id order or sorted by columns
//! (COLUMNSTORE.ORDER), each column's values
//! in chunks (`columns.rs`) on pages of their own, or, of the hybrid layout
//! (COLUMNSTORE.LAYOUT = 'PAX'), a page a range of rows holding a chunk of
//! each column (`group/pax.rs`); for each chunk, the rows
//! it holds, its least and greatest values and its nulls, so a scan reads
//! only the chunks a condition may match (zone maps), and of the columns
//! COLUMNSTORE.BLOOM names a Bloom filter of each part of it (`bloom.rs`),
//! and of each column of exact numbers, datetimes or character strings
//! the imprints of each part, unless COLUMNSTORE.IMPRINTS is OFF
//! (`imprints.rs`), and of the columns COLUMNSTORE.SKETCH names the codes
//! of their sketch (`sketch.rs`), on pages of their own too. A group is written
//! once and never changed: a change of one of its rows is the delta's.
//!
//! Its row ids are its least and greatest if every id between is its, in
//! order, else chunks of their own, each id 8 bytes, in its rows' order;
//! and if they are not in increasing order, its rows' positions by id too,
//! each an id and its position, 12 bytes, so a row is found by its id.
//! What describes it, its header, is kept on a chain of pages:
//!
//! ```text
//! page:    next page u64 (0 for none) | the header's bytes
//! header:  GROUP_FORMAT u8 | first id u64 | last id u64 | rows u32 |
//!          order u32 | each: column u32 | layout u8 (1 of the hybrid) |
//!          id runs u32 | each: run | by id runs u32 | each: run |
//!          columns u32 | each: runs u32 | each:
//!          run | nulls u32 | bounds u8 (1 if any) | least | greatest |
//!          parts u32 | each, of PART_ROWS rows: nulls u32 | bounds u8 |
//!          least | greatest | filter u8 (1 if any) | page u64 | at u32 |
//!          bytes u32 | imprints u8 (1 if any) | page u64 | at u32 |
//!          bytes u32 | sketch u8 (1 if any) | page u64 | at u32 |
//!          bytes u32;
//!          then of each column: edges u32 | each: bound;
//!          then sketched u32 | each: column u32;
//!          then of each column: sketch edges u32 | each: point u8 | bound
//! run:     first row u32 | rows u32 | page u64 | at u32 | len u32 (0
//!          for the whole page)
//! bound:   length u32 | the value as its type writes it in a record
//! ```
//!
//! Integers are little-endian. Every page passes through the table's
//! encodings, its page number their context.

mod header;
mod ids;
mod pax;
mod summary;
mod zones;

use std::borrow::Cow;

pub use header::{put_summary, read, read_summaries};
use ids::{write_by_id, write_ids};
pub use summary::{Overall, Summary};
use zones::{bounds_of, put_filters, write_column, zone_of};

use super::{Chunk, imprints, sketch};
use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::pager::PageId;
use crate::error::{Code, Result, SqlError};
use crate::storage::{Row, RowId};
use crate::value::{DataType, Value};

/// The format of a group's header.
const GROUP_FORMAT: u8 = 1;
/// Bytes of a row id.
const ID: usize = 8;
/// Bytes of a row id and its position.
const BY_ID: usize = 12;
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
    /// Whether chunks written are compressed (`columns/encoded.rs`).
    pub compress: bool,
    /// Whether groups are written in the hybrid layout (`Group::pax`).
    pub pax: bool,
    /// The kernels its chunks are decoded and judged on (RULES.md D570).
    pub kernels: &'a dyn crate::kernels::Kernels,
    /// Whether groups are written with imprints of the columns that take
    /// them (`columns/imprints.rs`).
    pub imprints: bool,
}

impl<S: Seal> Pages<'_, S> {
    /// A new page holding `content`, sealed.
    pub(super) fn put(&self, content: &[u8], written: &mut Vec<PageId>) -> Result<PageId> {
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
        self.with_told(page, |content, _| read(content))
    }

    /// As `with`, `read` told whether the content is read in the pool,
    /// the page having passed through no encoding: it is then read again
    /// as cheaply, with no copy to keep.
    fn with_told<T>(&self, page: PageId, read: impl FnOnce(&[u8], bool) -> Result<T>) -> Result<T> {
        let stored = self.pool.read(self.file, page.0)?;
        let opened = self.sealing.open(page.0, &stored)?;
        let in_place = matches!(opened, Cow::Borrowed(_));
        read(&opened, in_place)
    }

    /// What `read` makes of the bytes of `run` (`Run::at`), as `with`.
    ///
    /// # Errors
    ///
    /// As `with`; SQLCODE -916 if the run is past its page's end.
    pub fn with_run<T>(&self, run: &Run, read: impl FnOnce(&[u8]) -> Result<T>) -> Result<T> {
        self.with_run_told(run, |bytes, _| read(bytes))
    }

    /// As `with_run`, `read` told whether the bytes are read in the pool
    /// (`with_told`).
    ///
    /// # Errors
    ///
    /// As `with_run`.
    pub fn with_run_told<T>(&self, run: &Run, read: impl FnOnce(&[u8], bool) -> Result<T>) -> Result<T> {
        if run.len == 0 {
            return self.with_told(run.page, read);
        }
        let (at, len) = (usize::try_from(run.at).unwrap_or(usize::MAX), usize::try_from(run.len).unwrap_or(usize::MAX));
        self.with_told(run.page, |bytes, in_place| {
            read(bytes.get(at..at.saturating_add(len)).ok_or_else(|| corrupt("a chunk is past its page's end"))?, in_place)
        })
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
    /// Where on it they are, `len` bytes from `at`: of no bytes, the
    /// whole page, a chunk of a column on a page of its own; of the hybrid
    /// layout, a page holds a chunk of each column (`pax.rs`).
    pub at: u32,
    /// Their bytes from `at`; none, the whole page.
    pub len: u32,
}

impl Run {
    /// A run on page `page` whole.
    pub(super) fn whole(first: u32, rows: u32, page: PageId) -> Run {
        Run { first, rows, page, at: 0, len: 0 }
    }
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
    /// Of each `PART_ROWS` of its rows, in order, the last fewer, what a
    /// scan may skip them by: a chunk compressed holds many more rows than
    /// a scan should skip at once.
    pub parts: Vec<Part>,
}

/// \[DESIGN\] The rows of a chunk a scan skips at once at most: 1024, as a
/// zone of the row store's.
pub const PART_ROWS: u32 = 1024;

/// Of `PART_ROWS` rows of a chunk, what a scan may skip them by.
#[derive(Debug, Clone)]
pub struct Part {
    /// How many of their values are null.
    pub nulls: u32,
    /// Their least and greatest values that are not null, if any.
    pub bounds: Option<(Value, Value)>,
    /// Where their Bloom filter is, if their column has them.
    pub filter: Option<Filter>,
    /// Where their imprints are, if their column has them
    /// (`columns/imprints.rs`).
    pub imprint: Option<Filter>,
    /// Where the codes of their sketch are, if their column has one
    /// (`columns/sketch.rs`).
    pub sketch: Option<Filter>,
}

/// Where a part's Bloom filter is: `len` bytes from `at` of page `page`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Filter {
    /// The page.
    pub page: PageId,
    /// Where on it the filter begins.
    pub at: u32,
    /// Its bytes.
    pub len: u32,
}

/// \[DESIGN\] A frozen row group: its row ids, and its columns' chunks.
#[derive(Debug, Clone)]
pub struct Group {
    /// Its least row id.
    pub first: RowId,
    /// Its greatest row id.
    pub last: RowId,
    /// How many rows it holds.
    pub rows: u32,
    /// The chunks of its row ids, in its rows' order; none if they are
    /// every id from first to last, in order.
    pub ids: Vec<Run>,
    /// The chunks of its rows' positions by id, if its ids are not in
    /// increasing order; none if they are.
    pub by_id: Vec<Run>,
    /// Each column's chunks, in order.
    pub columns: Vec<Vec<Zone>>,
    /// The places of the columns its rows are sorted by, ascending, as
    /// COLUMNSTORE.ORDER said as it was written; none, in row id order.
    pub order: Vec<usize>,
    /// Whether it was written in the hybrid layout, as COLUMNSTORE.LAYOUT
    /// said as it was written: a page a range of its rows, a chunk of each
    /// column on it (`write_pax`).
    pub pax: bool,
    /// Of each column, the edges of its imprints' bins, none if it has no
    /// imprints (`columns/imprints.rs`).
    pub edges: Vec<Vec<Value>>,
    /// The places of the columns COLUMNSTORE.SKETCH named as it was
    /// written, in order.
    pub sketched: Vec<usize>,
    /// Of each column, the edges of its sketch, none if it has none
    /// (`columns/sketch.rs`).
    pub sketches: Vec<sketch::Edges>,
    /// The pages of its header.
    pub header: Vec<PageId>,
}

fn count32(n: usize) -> Result<u32> {
    u32::try_from(n).map_err(|_| too_large())
}

impl Group {
    /// Whether it has imprints of any column (`imprints.rs`).
    pub fn imprinted(&self) -> bool {
        self.edges.iter().any(|e| !e.is_empty())
    }

    /// Writes `rows`, whose ids are `ids`, a column of each of `types`, as
    /// a group, in their order.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the rows are not of the types; -900 if there are too
    /// many; an error of the pages; nothing written is kept. Its rows are
    /// in the order of the columns at `order`, those at `bloom` have
    /// Bloom filters, and those at `named` sketches, if they take them.
    pub fn write<S: Seal>(
        pages: &Pages<'_, S>,
        types: &[DataType],
        (ids, rows): (&[RowId], &[Row]),
        (order, bloom, named): (&[usize], &[usize], &[usize]),
    ) -> Result<Group> {
        let mut written = Vec::new();
        let made = Self::write_all(pages, types, (ids, rows), (order, bloom, named), &mut written);
        if made.is_err() {
            pages.free(&written);
        }
        made
    }

    fn write_all<S: Seal>(
        pages: &Pages<'_, S>,
        types: &[DataType],
        (ids, rows): (&[RowId], &[Row]),
        (order, bloom, named): (&[usize], &[usize], &[usize]),
        written: &mut Vec<PageId>,
    ) -> Result<Group> {
        let (Some(&first), Some(&last)) = (ids.iter().min(), ids.iter().max()) else {
            return Err(SqlError::new(Code::Api, "[DESIGN] a frozen row group of no rows"));
        };
        let id_runs = write_ids(pages, ids, written)?;
        let by_id = write_by_id(pages, ids, written)?;
        let mut columns = if pages.pax {
            pax::write_pax(pages, types, (rows, bloom), written)?
        } else {
            types
                .iter()
                .enumerate()
                .map(|(c, t)| write_column(pages, t, (rows, c, bloom.contains(&c)), written))
                .collect::<Result<Vec<_>>>()?
        };
        let edges = imprints::put_all(pages, (types, rows), &mut columns, written)?;
        let sketches = sketch::put_all(pages, (types, rows, named), &mut columns, written)?;
        let mut group = Group {
            first,
            last,
            rows: count32(rows.len())?,
            ids: id_runs,
            by_id,
            columns,
            order: order.to_vec(),
            pax: pages.pax,
            edges,
            sketched: named.to_vec(),
            sketches,
            header: Vec::new(),
        };
        group.header = put_chain(pages, &header::encode(&group, types)?, written)?;
        Ok(group)
    }

    /// Every page it is on, each once: chunks of the hybrid layout, and
    /// filters, share pages.
    pub fn pages(&self) -> Vec<PageId> {
        let chunks = self.columns.iter().flatten().map(|z| z.run.page);
        let ids = self.ids.iter().chain(&self.by_id).map(|r| r.page);
        let parts = self.columns.iter().flatten().flat_map(|z| &z.parts);
        let filters = parts.flat_map(|p| p.filter.into_iter().chain(p.imprint).chain(p.sketch)).map(|f| f.page);
        let mut all: Vec<PageId> = ids.chain(chunks).chain(filters).chain(self.header.iter().copied()).collect();
        all.sort_unstable_by_key(|p| p.0);
        all.dedup();
        all
    }

    /// The places of the columns it has Bloom filters of, in order.
    pub fn blooms(&self) -> Vec<usize> {
        let filtered = |zones: &Vec<Zone>| zones.iter().flat_map(|z| &z.parts).any(|p| p.filter.is_some());
        self.columns.iter().enumerate().filter(|(_, zones)| filtered(zones)).map(|(c, _)| c).collect()
    }

    /// Its row ids, in its rows' order.
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
        if !self.by_id.is_empty() {
            return self.position_by_id(pages, id);
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

    /// Whether its rows are in another order than their ids'.
    pub fn sorted(&self) -> bool {
        !self.by_id.is_empty()
    }

    /// The position of row `id`, found by id, if it is there.
    fn position_by_id<S: Seal>(&self, pages: &Pages<'_, S>, id: RowId) -> Result<Option<u32>> {
        for run in &self.by_id {
            let found = pages.with(run.page, |bytes| {
                let (entries, rest) = bytes.as_chunks::<BY_ID>();
                if !rest.is_empty() {
                    return Err(corrupt("a page of positions by id is not whole"));
                }
                let id_of = |e: &[u8; BY_ID]| e.first_chunk::<ID>().map_or(0, |b| u64::from_le_bytes(*b));
                if entries.last().is_none_or(|e| id_of(e) < id) {
                    return Ok(None);
                }
                let at = entries.binary_search_by_key(&id, id_of).ok().and_then(|n| entries.get(n));
                Ok(Some(at.and_then(|e| e.last_chunk::<4>()).map(|b| u32::from_le_bytes(*b))))
            })?;
            if let Some(found) = found {
                return Ok(found);
            }
        }
        Ok(None)
    }

    /// The value of column `column`, of type `t`, of the row at `position`.
    ///
    /// # Errors
    ///
    /// As `Pages::get`; SQLCODE -916 if its chunk is malformed.
    pub fn value<S: Seal>(&self, pages: &Pages<'_, S>, (column, t): (usize, &DataType), position: u32) -> Result<Value> {
        let zones = self.columns.get(column).ok_or_else(|| corrupt("a column is missing"))?;
        // Its chunks are in the order of their rows, so the one holding
        // the row is found by halving (RULES.md D1091).
        let zone = zones
            .get(zones.partition_point(|z| z.run.first.saturating_add(z.run.rows) <= position))
            .filter(|z| position >= z.run.first)
            .ok_or_else(|| corrupt("a row is in no chunk"))?;
        let at = usize::try_from(position.saturating_sub(zone.run.first)).unwrap_or(usize::MAX);
        pages.with_run(&zone.run, |bytes| Chunk::parse(bytes)?.value(at, t))
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
