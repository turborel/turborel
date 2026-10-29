//! \[DESIGN\] A row-store table's frozen row groups (RULES.md D31): the
//! COLUMNS layout. Rows are written to the table's tree, its delta, as a
//! table of the ROWS layout keeps them; once committed before every
//! snapshot, they are frozen into groups of their columns' values
//! (`engine/columns`), and their entries taken out of the tree.
//!
//! A row's newest state is its entry in the tree, if it has one; else, if
//! a group holds it, its value there, which every reader sees, as it was
//! committed before each one's snapshot. So a change of a frozen row is
//! made as of any row's, over a version committed before every snapshot
//! holding its frozen value (`frozen_version`); and a frozen row deleted
//! keeps its deletion in the tree, so the group's row is not seen again,
//! until a rewrite of the group drops it.
//!
//! Groups are found through a directory, each group's range of row ids
//! apart from the others'. A reader takes the directory as it is when it
//! begins; the tree's entries of rows frozen are taken out only once no
//! reader holds a directory without their group (`freeze.rs`), so no
//! reader misses a row. Its groups' header pages are listed on a chain of
//! pages, the first named by the table's meta page:
//!
//! ```text
//! directory:  groups u32 | each: the first page of its header u64
//! ```

mod batches;
mod freeze;
mod merge;
mod rewrite;

use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

pub(super) use batches::Batched;
pub(super) use merge::{Merged, Merging};

use super::options::Columns;
use super::sealing::Sealing;
use super::version::{Maker, Version};
use super::{RowTable, corrupt};
use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::pager::PageId;
use crate::engine::columns::group::{self, Group, Pages, Seal};
use crate::error::Result;
use crate::storage::{Row, RowId};
use crate::sync::{lock_builtin, read};
use crate::txn::Timestamp;
use crate::value::Value;

/// \[DESIGN\] The cost of a frozen row scanned for none of its columns, a
/// row of the tree's costing 1, and the cost more if all are read
/// (`bench/analytics.rs`, a pool holding the table: a COUNT(*) read a row at
/// a time 0.24 of the ROWS table's time, two columns of sixteen 0.32).
const FROZEN_ROW: f64 = 0.25;
const FROZEN_COLUMNS: f64 = 0.75;
/// \[DESIGN\] The share by which fetching a frozen row costs more than a
/// row of the tree, each column a page of its own (measured 8% slower).
const FROZEN_FETCH: f64 = 0.1;

/// A frozen page's context for its encodings: its number, the high bit
/// set, apart from the row ids that are the contexts of rows' values.
const FROZEN: u64 = 1 << 63;
/// Bytes of a chain page's link (`columns/group.rs`).
const LINK: usize = 8;

impl Seal for Sealing {
    fn seal(&self, page: u64, content: &[u8]) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(content.len());
        self.value(page | FROZEN, content, &mut out)?;
        Ok(out)
    }

    fn open<'a>(&self, page: u64, stored: &'a [u8]) -> Result<Cow<'a, [u8]>> {
        self.plain_value(page | FROZEN, stored)
    }
}

/// A group of the directory: freed once retired and no reader holds it.
pub(super) struct Entry {
    pub(super) group: Group,
    pool: Arc<BufferPool>,
    file: FileId,
    retired: AtomicBool,
}

impl Drop for Entry {
    fn drop(&mut self) {
        if self.retired.load(Ordering::Acquire) {
            for page in self.group.pages() {
                let _ = self.pool.free(self.file, page);
            }
        }
    }
}

/// The groups of a table, in row id order.
#[derive(Default)]
pub(super) struct Directory {
    pub(super) groups: Vec<Arc<Entry>>,
}

impl Directory {
    /// The group whose range holds row `id`, if any.
    pub(super) fn holding(&self, id: RowId) -> Option<&Arc<Entry>> {
        let at = self.groups.partition_point(|e| e.group.last < id);
        self.groups.get(at).filter(|e| e.group.first <= id)
    }

    /// The last row id of its last group.
    fn last(&self) -> Option<RowId> {
        self.groups.last().map(|e| e.group.last)
    }

    /// How many rows its groups hold.
    pub(super) fn rows(&self) -> u64 {
        self.groups.iter().map(|e| u64::from(e.group.rows)).sum()
    }
}

/// The tree's entries of rows frozen, taken out once no reader holds a
/// directory from before their group.
struct Retiring {
    before: std::sync::Weak<Directory>,
    entries: Vec<(RowId, Vec<u8>)>,
}

/// \[DESIGN\] A table's frozen row groups, and how its rows are frozen.
pub(super) struct Frozen {
    /// How its rows are frozen, if its layout is COLUMNS.
    columns: RwLock<Option<Columns>>,
    directory: RwLock<Arc<Directory>>,
    /// The first page of the directory as stored, 0 for none.
    stored: AtomicU64,
    /// Held while rows are frozen, one freeze at a time.
    freezing: Mutex<()>,
    retiring: Mutex<Vec<Retiring>>,
    /// Commit timestamps, each with when it was, so a freeze waits out
    /// COLUMNS.FREEZE_DELAY (`freeze.rs`).
    commits: Mutex<std::collections::VecDeque<(Timestamp, Instant)>>,
    /// When a freeze was last tried, so one due is not tried on every
    /// transaction's end.
    tried: Mutex<Option<Instant>>,
    /// The frozen rows given an entry of the tree since groups were last
    /// looked at for a rewrite (`rewrite.rs`).
    shadowed: AtomicU64,
}

impl Frozen {
    /// A table of no groups, freezing as `columns` says, if at all.
    pub(super) fn new(columns: Option<Columns>) -> Frozen {
        Frozen {
            columns: RwLock::new(columns),
            directory: RwLock::default(),
            stored: AtomicU64::new(0),
            freezing: Mutex::new(()),
            retiring: Mutex::default(),
            commits: Mutex::default(),
            tried: Mutex::new(None),
            shadowed: AtomicU64::new(0),
        }
    }

    /// Notes a frozen row given an entry of the tree: changed or deleted.
    pub(super) fn shadow(&self) {
        self.shadowed.fetch_add(1, Ordering::AcqRel);
    }

    /// The directory, as it is now.
    pub(super) fn directory(&self) -> Result<Arc<Directory>> {
        Ok(Arc::clone(&*read(&self.directory)?))
    }

    /// The first page of the directory as stored, 0 for none.
    pub(super) fn stored(&self) -> u64 {
        self.stored.load(Ordering::Acquire)
    }

    /// How its rows are frozen, if they are.
    pub(super) fn columns(&self) -> Option<Columns> {
        read(&self.columns).ok().and_then(|c| *c)
    }

    /// How its rows are frozen now.
    pub(super) fn reconfigure(&self, columns: Option<Columns>) {
        if let Ok(mut held) = self.columns.write() {
            *held = columns;
        }
    }

    /// Marks every group retired, so its pages are freed when the last
    /// reader is done: the table is dropped.
    pub(super) fn retire_all(&self) {
        if let Ok(directory) = read(&self.directory) {
            directory.groups.iter().for_each(|e| e.retired.store(true, Ordering::Release));
        }
    }
}

impl RowTable {
    /// Where its groups' pages are, sealed as its rows are.
    pub(super) fn group_pages(&self) -> Result<Pages<'_, Sealing>> {
        let capacity = self.pool.capacity(self.file)?.saturating_sub(self.sealing.overhead()).saturating_sub(LINK);
        Ok(Pages { pool: &self.pool, file: self.file, capacity, sealing: &self.sealing })
    }

    /// Row `id`'s frozen values, `columns` of them or all, if a group of
    /// `directory` holds it.
    pub(super) fn frozen_in(&self, directory: &Directory, id: RowId, columns: Option<&[usize]>) -> Result<Option<Row>> {
        let Some(entry) = directory.holding(id) else { return Ok(None) };
        let pages = self.group_pages()?;
        let Some(position) = entry.group.position(&pages, id)? else { return Ok(None) };
        let types = self.format.types();
        let mut row = vec![Value::Null; types.len()];
        let all: Vec<usize> = (0..types.len()).collect();
        for &c in columns.unwrap_or(&all) {
            if let (Some(slot), Some(t)) = (row.get_mut(c), types.get(c)) {
                *slot = entry.group.value(&pages, (c, t), position)?;
            }
        }
        Ok(Some(row))
    }

    /// Row `id`'s frozen values, if a group holds it now.
    pub(super) fn frozen_row(&self, id: RowId, columns: Option<&[usize]>) -> Result<Option<Row>> {
        self.frozen_in(&*self.frozen.directory()?, id, columns)
    }

    /// Whether a group's range holds row `id`: a deletion of it is kept.
    pub(super) fn frozen_holds(&self, id: RowId) -> bool {
        self.frozen.directory().is_ok_and(|d| d.holding(id).is_some())
    }

    /// A version of row `id`, committed before every snapshot, holding its
    /// frozen values, and the pages of its values out of line; none if no
    /// group holds it.
    pub(super) fn frozen_version(&self, id: RowId) -> Result<Option<(Vec<u8>, Vec<PageId>)>> {
        let Some(row) = self.frozen_row(id, None)? else { return Ok(None) };
        let (record, pages) = self.encode(id, &row)?;
        Ok(Some((Version { maker: Maker::Committed(0), deleted: false, older: None, record: &record }.encode(), pages)))
    }

    /// Loads the directory whose first page is `first`, as the meta page
    /// names it, 0 for none.
    pub(super) fn load_directory(&self, first: u64) -> Result<()> {
        if first == 0 {
            return Ok(());
        }
        let pages = self.group_pages()?;
        let (bytes, _) = group::read_chain(&pages, PageId(first))?;
        let (count, rest) = bytes.split_first_chunk::<4>().ok_or_else(|| corrupt("its groups' directory is too short"))?;
        let count = usize::try_from(u32::from_le_bytes(*count)).map_err(|_| corrupt("its groups' directory is too long"))?;
        if rest.len() != count.saturating_mul(8) {
            return Err(corrupt("its groups' directory is not whole"));
        }
        let mut groups = Vec::with_capacity(count);
        for b in rest.as_chunks::<8>().0 {
            let group = group::read(&pages, PageId(u64::from_le_bytes(*b)), self.format.types())?;
            groups.push(self.entry(group));
        }
        if groups.windows(2).any(|w| w.first().zip(w.get(1)).is_some_and(|(a, b)| a.group.last >= b.group.first)) {
            return Err(corrupt("its groups' row ids overlap"));
        }
        *crate::sync::write(&self.frozen.directory)? = Arc::new(Directory { groups });
        self.frozen.stored.store(first, Ordering::Release);
        Ok(())
    }

    /// `group`, as an entry of the directory.
    fn entry(&self, group: Group) -> Arc<Entry> {
        Arc::new(Entry { group, pool: Arc::clone(&self.pool), file: self.file, retired: AtomicBool::new(false) })
    }

    /// Stores `directory`, and names it in the meta page; frees the one
    /// stored before.
    fn store_directory(&self, directory: &Directory) -> Result<()> {
        let mut bytes = u32::try_from(directory.groups.len()).map_err(|_| corrupt("it has too many groups"))?.to_le_bytes().to_vec();
        for e in &directory.groups {
            let first = e.group.header.first().ok_or_else(|| corrupt("a group has no header"))?;
            bytes.extend_from_slice(&first.0.to_le_bytes());
        }
        let pages = self.group_pages()?;
        let chain = group::write_chain(&pages, &bytes)?;
        let first = chain.first().map_or(0, |p| p.0);
        let before = self.frozen.stored.swap(first, Ordering::AcqRel);
        self.write_meta()?;
        if before != 0 {
            let (_, old) = group::read_chain(&pages, PageId(before))?;
            pages.free(&old);
        }
        Ok(())
    }

    /// Gives `each` every frozen row of `directory` no entry of the tree
    /// stands for, every column of it, a vector at a time.
    pub(super) fn each_unshadowed(&self, directory: &Directory, mut each: impl FnMut(RowId, Row) -> Result<()>) -> Result<()> {
        let pages = self.group_pages()?;
        let types = self.format.types();
        let all: Vec<usize> = (0..types.len()).collect();
        for entry in &directory.groups {
            let ids = entry.group.row_ids(&pages)?;
            for (n, chunk) in ids.chunks(merge::VECTOR_ROWS).enumerate() {
                let from = u32::try_from(n.saturating_mul(merge::VECTOR_ROWS)).map_err(|_| corrupt("a group is too large"))?;
                let to = from.saturating_add(u32::try_from(chunk.len()).unwrap_or(u32::MAX));
                let found = crate::engine::columns::scan::rows(&entry.group, &pages, types, &all, (from, to))?;
                for (&id, row) in chunk.iter().zip(found) {
                    self.unless_shadowed(id, row, &mut each)?;
                }
            }
        }
        Ok(())
    }

    /// Gives `each` frozen row `id`, `row`, unless an entry of the tree
    /// stands for it.
    fn unless_shadowed(&self, id: RowId, row: Row, each: &mut impl FnMut(RowId, Row) -> Result<()>) -> Result<()> {
        if self.tree.get(&super::key(id))?.is_some() {
            return Ok(());
        }
        each(id, row)
    }

    /// The planner's cost of a scan of `columns` (`TableStorage::scan_cost`):
    /// each row of the tree 1, each frozen row as `FROZEN_ROW` and
    /// `FROZEN_COLUMNS` say.
    pub(super) fn frozen_scan_cost(&self, columns: &[usize]) -> f64 {
        let (frozen, all) = self.frozen_share();
        let width = f64::from(u32::try_from(self.format.columns()).unwrap_or(u32::MAX)).max(1.0_f64);
        let read = f64::from(u32::try_from(columns.len()).unwrap_or(u32::MAX)).min(width);
        (all - frozen).max(0.0_f64) + frozen * (FROZEN_ROW + FROZEN_COLUMNS * read / width)
    }

    /// The planner's cost of a row fetched by id (`TableStorage::fetch_cost`).
    pub(super) fn frozen_fetch_cost(&self) -> f64 {
        let (frozen, all) = self.frozen_share();
        let share = if all > 0.0_f64 { (frozen / all).min(1.0_f64) } else { 0.0_f64 };
        crate::storage::DEFAULT_FETCH_COST * (1.0_f64 + FROZEN_FETCH * share)
    }

    /// The rows frozen, and all the table's rows, as costs.
    fn frozen_share(&self) -> (f64, f64) {
        let frozen = self.frozen.directory().map_or(0, |d| d.rows());
        (crate::storage::cost_of(usize::try_from(frozen).unwrap_or(usize::MAX)), crate::storage::cost_of(self.live.load(Ordering::Acquire)))
    }

    /// Notes a commit at `at`, for COLUMNS.FREEZE_DELAY.
    pub(super) fn note_commit(&self, at: Timestamp) {
        let Some(columns) = read(&self.frozen.columns).ok().and_then(|c| *c) else { return };
        if columns.freeze_delay > 0 {
            freeze::noted(&mut lock_builtin(&self.frozen.commits), at, columns.freeze_delay);
        }
    }
}

// Unit tests: the directory finds the group holding a row id, or none
// (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
