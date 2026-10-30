//! \[DESIGN\] A row-store table's frozen row groups (RULES.md D31): the
//! column store. Rows are written to the table's tree, its delta, as a
//! table of the row store keeps them; once committed before every
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
//! reader misses a row. Of each group the directory holds in memory only
//! its summary (`group/summary.rs`), so a scan passes over a group no
//! condition may match without its header; a header is read as it is
//! needed, and held while it was read recently (`columns/cache.rs`). The
//! summaries are kept on a chain of pages, the first named by the table's
//! meta page:
//!
//! ```text
//! directory:  groups u32 | each: its summary (`group/header.rs`)
//! ```

mod batches;
mod freeze;
mod merge;
mod rewrite;

use std::borrow::Cow;
use std::sync::atomic::{AtomicU64, Ordering};
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
use crate::engine::columns::cache::HeaderCache;
use crate::engine::columns::group::{self, Group, Pages, Seal, Summary};
use crate::engine::columns::scan::candidates;
use crate::error::Result;
use crate::storage::{Conjunction, Row, RowId};
use crate::sync::{lock_builtin, read};
use crate::txn::Timestamp;
use crate::value::Value;

/// \[DESIGN\] The cost of a row of the tree scanned for none of its
/// columns, and the cost more if all are read: a row of all costing 1. Of
/// 400,000 rows of sixteen columns, a pool holding them, a COUNT(*) read a
/// row in 0.27 us, fourteen columns more 0.06 us besides their sums.
const ROW: f64 = 0.8;
const ROW_COLUMNS: f64 = 0.2;
/// \[DESIGN\] The cost of a row fetched by id through an index, a row of
/// all columns scanned costing 1: of a table the buffer pool holds, and
/// more as it holds less of it, to that of one it holds none of. Of
/// 400,000 rows of sixteen columns in a database's files, a fetch took
/// 1.28 us a row with a pool of 256 MiB, a scan 0.29; with one of 1 MiB,
/// the file in the system's cache, 5.3 and 0.36.
const ROW_FETCH_HELD: f64 = 4.4;
const ROW_FETCH_READ: f64 = 14.6;
/// \[DESIGN\] Bytes of a row's entry in the tree besides its record: its
/// key, its version and its slot.
const ENTRY_BYTES: f64 = 32.0;
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

/// A group of the directory, its summary: its pages freed once retired
/// and no reader holds it.
pub(super) struct Entry {
    pub(super) summary: Summary,
    headers: Arc<HeaderCache>,
    pool: Arc<BufferPool>,
    file: FileId,
    /// Its pages, once it is retired.
    retired: Mutex<Option<Vec<PageId>>>,
}

impl Drop for Entry {
    fn drop(&mut self) {
        self.headers.forget((self.file, self.summary.header));
        if let Some(pages) = lock_builtin(&self.retired).take() {
            for page in pages {
                let _ = self.pool.free(self.file, page);
            }
        }
    }
}

impl Entry {
    /// Marks it retired, `pages`, its group's, freed when the last reader
    /// is done.
    fn retire(&self, pages: Vec<PageId>) {
        *lock_builtin(&self.retired) = Some(pages);
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
        let at = self.groups.partition_point(|e| e.summary.last < id);
        self.groups.get(at).filter(|e| e.summary.first <= id)
    }

    /// The last row id of its last group.
    fn last(&self) -> Option<RowId> {
        self.groups.last().map(|e| e.summary.last)
    }

    /// How many rows its groups hold.
    pub(super) fn rows(&self) -> u64 {
        self.groups.iter().map(|e| u64::from(e.summary.rows)).sum()
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
    /// How its rows are frozen, if it is the column store's.
    columns: RwLock<Option<Columns>>,
    directory: RwLock<Arc<Directory>>,
    /// The first page of the directory as stored, 0 for none.
    stored: AtomicU64,
    /// Held while rows are frozen, one freeze at a time.
    freezing: Mutex<()>,
    retiring: Mutex<Vec<Retiring>>,
    /// Commit timestamps, each with when it was, so a freeze waits out
    /// COLUMNSTORE.FREEZE_DELAY (`freeze.rs`).
    commits: Mutex<std::collections::VecDeque<(Timestamp, Instant)>>,
    /// When a freeze was last tried, so one due is not tried on every
    /// transaction's end.
    tried: Mutex<Option<Instant>>,
    /// The frozen rows given an entry of the tree since groups were last
    /// looked at for a rewrite (`rewrite.rs`).
    shadowed: AtomicU64,
    /// The least row id a freeze under way may freeze, `NOT_FREEZING` if
    /// none is: a deletion of a row from it on is kept (`freeze.rs`).
    freezing_from: AtomicU64,
}

/// What `Frozen::freezing_from` holds while no freeze is under way.
const NOT_FREEZING: u64 = u64::MAX;

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
            freezing_from: AtomicU64::new(NOT_FREEZING),
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
}

impl RowTable {
    /// Where its groups' pages are, sealed as its rows are.
    pub(super) fn group_pages(&self) -> Result<Pages<'_, Sealing>> {
        let capacity = self.pool.capacity(self.file)?.saturating_sub(self.sealing.overhead()).saturating_sub(LINK);
        let compress = self.frozen.columns().is_none_or(|c| c.compress);
        Ok(Pages { pool: &self.pool, file: self.file, capacity, sealing: &self.sealing, compress })
    }

    /// The group of `entry`: its header as held, else read and held.
    ///
    /// # Errors
    ///
    /// As `group::read`.
    pub(super) fn group_of(&self, entry: &Entry) -> Result<Arc<Group>> {
        let key = (self.file, entry.summary.header);
        if let Some(group) = self.headers.get(key) {
            return Ok(group);
        }
        let group = Arc::new(group::read(&self.group_pages()?, entry.summary.header, self.format.types())?);
        self.headers.put(key, &group);
        Ok(group)
    }

    /// Marks every group retired, so its pages are freed when the last
    /// reader is done: the table is dropped.
    pub(super) fn retire_all(&self) -> Result<()> {
        for entry in &self.frozen.directory()?.groups {
            entry.retire(self.group_of(entry)?.pages());
        }
        Ok(())
    }

    /// Row `id`'s frozen values, `columns` of them or all, if a group of
    /// `directory` holds it.
    pub(super) fn frozen_in(&self, directory: &Directory, id: RowId, columns: Option<&[usize]>) -> Result<Option<Row>> {
        let Some(entry) = directory.holding(id) else { return Ok(None) };
        let pages = self.group_pages()?;
        let group = self.group_of(entry)?;
        let Some(position) = group.position(&pages, id)? else { return Ok(None) };
        let types = self.format.types();
        let mut row = vec![Value::Null; types.len()];
        let all: Vec<usize> = (0..types.len()).collect();
        for &c in columns.unwrap_or(&all) {
            if let (Some(slot), Some(t)) = (row.get_mut(c), types.get(c)) {
                *slot = group.value(&pages, (c, t), position)?;
            }
        }
        Ok(Some(row))
    }

    /// Row `id`'s frozen values, if a group holds it now.
    pub(super) fn frozen_row(&self, id: RowId, columns: Option<&[usize]>) -> Result<Option<Row>> {
        self.frozen_in(&*self.frozen.directory()?, id, columns)
    }

    /// Whether a group's range holds row `id`, or a freeze under way may
    /// freeze it: a deletion of it is kept.
    pub(super) fn frozen_holds(&self, id: RowId) -> bool {
        id >= self.frozen.freezing_from.load(Ordering::SeqCst) || self.frozen.directory().is_ok_and(|d| d.holding(id).is_some())
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
        let summaries = group::read_summaries(rest, self.format.types())?;
        if summaries.len() != count {
            return Err(corrupt("its groups' directory is not whole"));
        }
        let groups: Vec<Arc<Entry>> = summaries.into_iter().map(|s| self.entry(s)).collect();
        if groups.windows(2).any(|w| w.first().zip(w.get(1)).is_some_and(|(a, b)| a.summary.last >= b.summary.first)) {
            return Err(corrupt("its groups' row ids overlap"));
        }
        *crate::sync::write(&self.frozen.directory)? = Arc::new(Directory { groups });
        self.frozen.stored.store(first, Ordering::Release);
        Ok(())
    }

    /// The group `summary` sums up, as an entry of the directory.
    fn entry(&self, summary: Summary) -> Arc<Entry> {
        Arc::new(Entry {
            summary,
            headers: Arc::clone(&self.headers),
            pool: Arc::clone(&self.pool),
            file: self.file,
            retired: Mutex::new(None),
        })
    }

    /// `group`, just written, as an entry of the directory, its header held.
    fn entry_of(&self, group: Group) -> Result<Arc<Entry>> {
        let header = *group.header.first().ok_or_else(|| corrupt("a group has no header"))?;
        let entry = self.entry(Summary::of(&group, header));
        self.headers.put((self.file, header), &Arc::new(group));
        Ok(entry)
    }

    /// Stores `directory`, and names it in the meta page; frees the one
    /// stored before.
    fn store_directory(&self, directory: &Directory) -> Result<()> {
        let mut bytes = u32::try_from(directory.groups.len()).map_err(|_| corrupt("it has too many groups"))?.to_le_bytes().to_vec();
        for e in &directory.groups {
            group::put_summary(&mut bytes, &e.summary, self.format.types())?;
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
            let group = self.group_of(entry)?;
            let ids = group.row_ids(&pages)?;
            for (n, chunk) in ids.chunks(merge::VECTOR_ROWS).enumerate() {
                let from = u32::try_from(n.saturating_mul(merge::VECTOR_ROWS)).map_err(|_| corrupt("a group is too large"))?;
                let to = from.saturating_add(u32::try_from(chunk.len()).unwrap_or(u32::MAX));
                let found = crate::engine::columns::scan::rows(&group, &pages, types, &all, (from, to))?;
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
    /// each row of the tree as `ROW` and `ROW_COLUMNS` say, each frozen row
    /// as `FROZEN_ROW` and `FROZEN_COLUMNS`.
    pub(super) fn frozen_scan_cost(&self, columns: &[usize]) -> f64 {
        let (frozen, all) = self.frozen_share();
        let width = f64::from(u32::try_from(self.format.columns()).unwrap_or(u32::MAX)).max(1.0_f64);
        let read = f64::from(u32::try_from(columns.len()).unwrap_or(u32::MAX)).min(width) / width;
        (all - frozen).max(0.0_f64) * (ROW + ROW_COLUMNS * read) + frozen * (FROZEN_ROW + FROZEN_COLUMNS * read)
    }

    /// The share of the table's rows a scan told `conditions` reads
    /// (`TableStorage::scan_share`): every row of the tree, and the frozen
    /// rows of the groups they may match, of the chunks they may match of
    /// a group whose header is held (`columns/scan.rs`).
    pub(super) fn frozen_read_share(&self, conditions: &Conjunction) -> f64 {
        let Ok(directory) = self.frozen.directory() else { return 1.0_f64 };
        let (frozen, all) = self.frozen_share();
        let read: u64 = directory
            .groups
            .iter()
            .filter(|e| e.summary.may_match(Some(conditions)))
            .map(|e| match self.headers.get((self.file, e.summary.header)) {
                Some(group) => candidates(&group, Some(conditions)).iter().map(|(from, to)| u64::from(to.saturating_sub(*from))).sum(),
                None => u64::from(e.summary.rows),
            })
            .sum();
        let read = crate::storage::cost_of(usize::try_from(read).unwrap_or(usize::MAX));
        if all > 0.0_f64 { ((all - frozen).max(0.0_f64) + read).min(all) / all } else { 1.0_f64 }
    }

    /// The share of the table's rows the buffer pool may hold: its bytes,
    /// of the rows' as their statistics' width says, else as a page each
    /// holds a hundred, all if it has no row.
    fn held_share(&self) -> f64 {
        let count = |n: usize| f64::from(u32::try_from(n).unwrap_or(u32::MAX));
        let page = count(self.pool.capacity(self.file).unwrap_or(4096));
        let pool = count(self.pool.frames()) * page;
        let width = self.gathered.get().map_or(page / 100.0_f64, |s| s.width.max(1.0_f64));
        let table = count(self.live.load(Ordering::Acquire)) * (width + ENTRY_BYTES);
        if table > 0.0_f64 { (pool / table).min(1.0_f64) } else { 1.0_f64 }
    }

    /// The planner's cost of a row fetched by id (`TableStorage::fetch_cost`).
    pub(super) fn frozen_fetch_cost(&self) -> f64 {
        let (frozen, all) = self.frozen_share();
        let share = if all > 0.0_f64 { (frozen / all).min(1.0_f64) } else { 0.0_f64 };
        let held = self.held_share();
        let fetch = ROW_FETCH_HELD + (ROW_FETCH_READ - ROW_FETCH_HELD) * (1.0_f64 - held);
        fetch * (1.0_f64 + FROZEN_FETCH * share)
    }

    /// The rows frozen, and all the table's rows, as costs.
    fn frozen_share(&self) -> (f64, f64) {
        let frozen = self.frozen.directory().map_or(0, |d| d.rows());
        (crate::storage::cost_of(usize::try_from(frozen).unwrap_or(usize::MAX)), crate::storage::cost_of(self.live.load(Ordering::Acquire)))
    }

    /// Notes a commit at `at`, for COLUMNSTORE.FREEZE_DELAY.
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
