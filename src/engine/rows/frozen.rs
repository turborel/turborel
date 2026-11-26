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
//! directory:  groups u32, the high bit set if each one's stamp u64
//!             follows (`stamped.rs`) | each: its summary
//!             (`group/header.rs`)
//! ```

mod batches;
pub(super) mod extents;
mod fetched;
mod freeze;
mod late;
mod merge;
mod ordered;
mod rewrite;
mod sampled;
pub(super) use sampled::Chosen;
mod loaded;
mod shadow_bits;
mod stamped;

use std::borrow::Cow;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

pub(super) use batches::Batched;
pub(super) use merge::{Merged, Merging};
pub(super) use ordered::Ordered;
use shadow_bits::ShadowBits;
pub(super) use stamped::Loads;

use super::options::Columns;
use super::sealing::Sealing;
use super::version::Version;
use super::{RowTable, corrupt};
use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::pager::PageId;
use crate::engine::columns::cache::{HeaderCache, Slot};
use crate::engine::columns::group::{self, Group, Pages, Seal, Summary};
use crate::engine::columns::scan::candidates;
pub(super) use crate::engine::tree::LEAST_LOOKUPS;
use crate::error::Result;
use crate::storage::{Conjunction, Row, RowId};
use crate::sync::{lock_builtin, read};
use crate::txn::Timestamp;

/// \[DESIGN\] The cost of a row of the tree scanned for none of its
/// columns, and the cost more if all are read: a row of all costing 1
/// (`bench/analytics.rs`).
const ROW: f64 = 0.8;
const ROW_COLUMNS: f64 = 0.2;
/// \[DESIGN\] The share by which a row of the tree scanned costs more if
/// the buffer pool missed its page, read from the system's cache
/// (`bench/kept.rs`, its weights).
pub(super) const ROW_SCAN_MISSED: f64 = 1.1;
/// \[DESIGN\] The cost of a row fetched by id through an index, a row of
/// all columns scanned costing 1 (`bench/kept.rs`, its weights): its page
/// held by the buffer pool; missed by the pool and read from the system's
/// cache; and read from the disk, that cost more by a page read from the
/// disk less one from the cache (`bench/pages.rs`), in rows scanned.
pub(super) const ROW_FETCH_HELD: f64 = 8.6;
pub(super) const ROW_FETCH_CACHED: f64 = 72.0;
pub(super) const ROW_FETCH_DISK: f64 = 5900.0;
/// \[DESIGN\] STORAGE.SYSTEM_CACHE_BYTES by default: the bytes of the
/// data file the system's cache is taken to hold beyond the pool. A
/// machine of the default budget (MEMORY.BUDGET_BYTES, 1 GiB) is taken to
/// leave its cache a few times as much; a file up to this past its pool is
/// priced as it was measured, in the cache.
pub const SYSTEM_CACHE_BYTES: u64 = 4 << 30;
/// \[DESIGN\] Bytes of a row's entry in the tree besides its record: its
/// key, its version and its slot.
const ENTRY_BYTES: f64 = 32.0;
/// \[DESIGN\] The cost of a frozen row scanned for none of its columns, a
/// row of the tree's costing 1, and the cost more if all are read
/// (`bench/analytics.rs`).
const FROZEN_ROW: f64 = 0.25;
const FROZEN_COLUMNS: f64 = 0.75;
/// \[DESIGN\] What fetching a frozen row costs, a row of the tree's
/// costing 1: `FROZEN_FETCH` to find its group and place, whatever is
/// read, and `FROZEN_FETCH_COLUMN` more for each column read, each a
/// chunk of its own on a page of its own: the line through one and four
/// columns read by scattered row ids, every header held, against the
/// tree's row, which eight and seventeen lie near (`bench/fetch.rs`,
/// RULES.md D1091, D1097).
const FROZEN_FETCH: f64 = 0.26;
const FROZEN_FETCH_COLUMN: f64 = 0.6;

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
    /// The rows of its range the tree may have entries for
    /// (`shadow_bits.rs`).
    pub(super) shadow: ShadowBits,
    headers: Arc<HeaderCache>,
    /// Its group's header, while the cache holds it (RULES.md D1090).
    slot: Arc<Slot>,
    pool: Arc<BufferPool>,
    file: FileId,
    /// Of a group written as an extent, its segment, held while the entry
    /// is, and where the extent starts in it (RULES.md D1146).
    extent: Option<(Arc<extents::Segment>, u64)>,
    /// Its pages, once it is retired.
    retired: Mutex<Option<Vec<PageId>>>,
    /// Who made it, if not committed before every snapshot (`stamped.rs`).
    made: AtomicU64,
}

impl Drop for Entry {
    fn drop(&mut self) {
        self.headers.forget(self.header_key());
        // An extent's pages are its segment's, given up with it.
        if let Some(pages) = lock_builtin(&self.retired).take().filter(|_| self.extent.is_none()) {
            for page in pages {
                let _ = self.pool.free(self.file, page);
            }
        }
    }
}

impl Entry {
    /// Where the pool holds its group's header: its file and first page.
    fn header_key(&self) -> (FileId, PageId) {
        (self.file, PageId(self.base().saturating_add(self.summary.header.0)))
    }

    /// Where its extent starts in its segment, 0 if it is on pages.
    fn base(&self) -> u64 {
        self.extent.as_ref().map_or(0, |(_, base)| *base)
    }

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
    /// Whether a group is not committed before every snapshot, so a reader
    /// takes the groups it sees (`stamped.rs`).
    pending: bool,
    /// The directory it is the part of, held with it.
    _whole: Option<Arc<Directory>>,
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

    /// Another of the same groups, so the readers of this one, the table's
    /// whole directory, are told apart from those that come after
    /// (`freeze.rs`, RULES.md D1140).
    fn again(&self) -> Directory {
        Directory { groups: self.groups.clone(), pending: self.pending, _whole: None }
    }
}

/// The tree's entries of rows frozen, taken out once no reader holds a
/// directory from before their group.
struct Retiring {
    before: std::sync::Weak<Directory>,
    entries: Vec<(RowId, Vec<u8>)>,
}

/// The pages of the values out of line of entries taken out, freed once
/// no reader holds the directory that stood as they were taken out.
struct Freeing {
    after: std::sync::Weak<Directory>,
    pages: Vec<PageId>,
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
    freeing: Mutex<Vec<Freeing>>,
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
    /// One past the last row id of its groups, 0 if it has none: a write
    /// of a row from it on marks no group's shadow bits.
    groups_end: AtomicU64,
    /// Whether it has never frozen rows nor held a group, as a table of
    /// the row store has not: its writes and commits then read this alone,
    /// not the locks of `columns` and `directory`, words every session
    /// writing the table would share. Never set again once clear.
    plain: std::sync::atomic::AtomicBool,
    /// Whether scans trust the shadow bits, as they do but in tests that
    /// read the tree over every group as if every bit were set.
    trust_bits: std::sync::atomic::AtomicBool,
    /// A load whose rows go straight to groups, and the groups recovery
    /// has yet to settle (`stamped.rs`).
    loads: Loads,
    /// The segments its loads write their groups to (`extents.rs`).
    pub(super) extents: Arc<extents::Extents>,
}

/// What `Frozen::freezing_from` holds while no freeze is under way.
const NOT_FREEZING: u64 = u64::MAX;

impl Frozen {
    /// A table of no groups, freezing as `columns` says, if at all.
    pub(super) fn new(columns: Option<Columns>) -> Frozen {
        Frozen {
            plain: std::sync::atomic::AtomicBool::new(columns.is_none()),
            columns: RwLock::new(columns),
            directory: RwLock::default(),
            stored: AtomicU64::new(0),
            freezing: Mutex::new(()),
            retiring: Mutex::default(),
            freeing: Mutex::default(),
            commits: Mutex::default(),
            tried: Mutex::new(None),
            shadowed: AtomicU64::new(0),
            freezing_from: AtomicU64::new(NOT_FREEZING),
            groups_end: AtomicU64::new(0),
            trust_bits: std::sync::atomic::AtomicBool::new(true),
            loads: Loads::default(),
            extents: Arc::default(),
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

    /// \[DESIGN\] The directory, held by a read by row id from before it
    /// finds the row's entry until it has fetched the values it keeps out
    /// of line, so a freeze taking the entry out meanwhile does not free
    /// them under it (`freeze.rs`, RULES.md D1140). None of a table that
    /// has never frozen rows, whose reads share no word for it.
    pub(super) fn pinned(&self) -> Option<Arc<Directory>> {
        if self.plain() { None } else { self.directory().ok() }
    }

    /// The first page of the directory as stored, 0 for none.
    pub(super) fn stored(&self) -> u64 {
        self.stored.load(Ordering::Acquire)
    }

    /// How its rows are frozen, if they are.
    pub(super) fn columns(&self) -> Option<Columns> {
        read(&self.columns).ok().and_then(|c| *c)
    }

    /// Whether it has never frozen rows nor held a group (`plain`).
    pub(super) fn plain(&self) -> bool {
        self.plain.load(Ordering::Acquire)
    }

    /// Notes its groups end before row `end`, 0 for no group.
    pub(super) fn end_groups(&self, end: RowId) {
        if end != 0 {
            self.plain.store(false, Ordering::Release);
        }
        self.groups_end.store(end, Ordering::SeqCst);
    }

    /// How its rows are frozen now.
    pub(super) fn reconfigure(&self, columns: Option<Columns>) {
        if columns.is_some() {
            self.plain.store(false, Ordering::Release);
        }
        if let Ok(mut held) = self.columns.write() {
            *held = columns;
        }
    }
}

impl RowTable {
    /// Where its groups' pages are, sealed as its rows are.
    pub(super) fn group_pages(&self) -> Result<Pages<'_, Sealing>> {
        let capacity = self.pool.capacity(self.file)?.saturating_sub(self.sealing.overhead()).saturating_sub(LINK);
        let columns = self.frozen.columns();
        let (compress, pax, imprints) =
            (columns.is_none_or(|c| c.compress), columns.is_some_and(|c| c.pax), columns.is_some_and(|c| c.imprints));
        Ok(Pages {
            pool: &self.pool,
            file: self.file,
            capacity,
            sealing: &self.sealing,
            compress,
            pax,
            imprints,
            kernels: &*self.kernels,
            base: 0,
            sink: None,
        })
    }

    /// The group of `entry`: its header as held, else read and held.
    ///
    /// # Errors
    ///
    /// As `group::read`.
    pub(super) fn group_of(&self, entry: &Entry) -> Result<Arc<Group>> {
        if let Some(group) = entry.slot.get() {
            return Ok(group);
        }
        let group = Arc::new(group::read(&self.pages_of(entry)?, entry.summary.header, self.format.types())?);
        self.headers.put(entry.header_key(), &entry.slot, &group);
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

    /// Row `id`'s frozen values, if a group holds it now.
    pub(super) fn frozen_row(&self, id: RowId, columns: Option<&[usize]>) -> Result<Option<Row>> {
        self.frozen_in(&*self.frozen.directory()?, id, columns)
    }

    /// Whether a group's range holds row `id`, or a freeze under way may
    /// freeze it: a deletion of it is kept.
    pub(super) fn frozen_holds(&self, id: RowId) -> bool {
        id >= self.frozen.freezing_from.load(Ordering::SeqCst) || self.frozen.directory().is_ok_and(|d| d.holding(id).is_some())
    }

    /// A version of row `id`, made as its group was (`stamped.rs`), holding
    /// its frozen values, and the pages of its values out of line; none if
    /// no group holds it.
    pub(super) fn frozen_version(&self, id: RowId) -> Result<Option<(Vec<u8>, Vec<PageId>)>> {
        let directory = self.frozen.directory()?;
        let Some(maker) = directory.holding(id).map(|e| self.group_maker(e)) else { return Ok(None) };
        let Some(row) = self.frozen_in(&directory, id, None)? else { return Ok(None) };
        let (record, pages) = self.encode(id, &row)?;
        Ok(Some((Version { maker, deleted: false, older: None, record: &record }.encode(), pages)))
    }

    /// Loads the directory whose first page is `first`, as the meta page
    /// names it, 0 for none.
    pub(super) fn load_directory(&self, first: u64) -> Result<()> {
        if first == 0 {
            return Ok(());
        }
        let pages = self.group_pages()?;
        let (bytes, _) = group::read_chain(&pages, PageId(first))?;
        let (count, stamps, rest) = stamped::counted(&bytes)?;
        let summaries = group::read_summaries(rest, self.format.types())?;
        if summaries.len() != count {
            return Err(corrupt("its groups' directory is not whole"));
        }
        let groups = self.unstamped(self.placed_entries(summaries)?, &stamps);
        if groups.windows(2).any(|w| w.first().zip(w.get(1)).is_some_and(|(a, b)| a.summary.last >= b.summary.first)) {
            return Err(corrupt("its groups' row ids overlap"));
        }
        let end = groups.last().map_or(0, |e| e.summary.last.saturating_add(1));
        *crate::sync::write(&self.frozen.directory)? = Arc::new(Directory::of(groups));
        self.frozen.end_groups(end);
        self.frozen.stored.store(first, Ordering::Release);
        self.mark_opened()
    }

    /// The group `summary` sums up, as an entry of the directory, in
    /// `extent`'s segment if given, else on its table's pages.
    fn entry_in(&self, summary: Summary, extent: Option<(Arc<extents::Segment>, u64)>) -> Arc<Entry> {
        Arc::new(Entry {
            shadow: ShadowBits::new(summary.first, summary.last),
            summary,
            headers: Arc::clone(&self.headers),
            slot: Arc::default(),
            pool: Arc::clone(&self.pool),
            file: extent.as_ref().map_or(self.file, |(segment, _)| segment.file),
            extent,
            retired: Mutex::new(None),
            made: AtomicU64::new(0),
        })
    }

    /// `group`, just written, as an entry of the directory, its header held.
    fn entry_of(&self, group: Group) -> Result<Arc<Entry>> {
        let header = *group.header.first().ok_or_else(|| corrupt("a group has no header"))?;
        let entry = self.entry_in(Summary::of(&group, header), None);
        self.headers.put(entry.header_key(), &entry.slot, &Arc::new(group));
        Ok(entry)
    }

    /// Stores `directory`, and names it in the meta page; frees the one
    /// stored before.
    fn store_directory(&self, directory: &Directory) -> Result<()> {
        self.frozen.extents.name(directory.groups.iter().map(|e| &e.summary));
        let mut bytes = stamped::count_of(directory)?;
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

    /// The planner's cost of a scan of `columns` (`TableStorage::scan_cost`):
    /// each row of the tree as `ROW` and `ROW_COLUMNS` say, `ROW_SCAN_MISSED`
    /// more of the share of its pages the pool does not hold
    /// (`held_share`); each frozen row as `FROZEN_ROW` and `FROZEN_COLUMNS`.
    pub(super) fn frozen_scan_cost(&self, columns: &[usize]) -> f64 {
        let (frozen, all) = self.frozen_share();
        let width = f64::from(u32::try_from(self.format.columns()).unwrap_or(u32::MAX)).max(1.0_f64);
        let read = f64::from(u32::try_from(columns.len()).unwrap_or(u32::MAX)).min(width) / width;
        let missed = 1.0_f64 + ROW_SCAN_MISSED * (1.0_f64 - self.held_share());
        (all - frozen).max(0.0_f64) * (ROW + ROW_COLUMNS * read) * missed + frozen * (FROZEN_ROW + FROZEN_COLUMNS * read)
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
            .map(|e| match e.slot.peek() {
                Some(group) => candidates(&group, Some(conditions)).iter().map(|(from, to)| u64::from(to.saturating_sub(*from))).sum(),
                None => u64::from(e.summary.rows),
            })
            .sum();
        let read = crate::storage::cost_of(usize::try_from(read).unwrap_or(usize::MAX));
        if all > 0.0_f64 { ((all - frozen).max(0.0_f64) + read).min(all) / all } else { 1.0_f64 }
    }

    /// The share of its fetches' pages the buffer pool holds: as its tree's
    /// lookups found them (`BLinkTree::missed_share`), once it has counted
    /// `LEAST_LOOKUPS`; before, the pool's bytes of the table's, by its
    /// statistics' width, else as a page each holds a hundred, all if it
    /// has no row.
    pub(super) fn held_share(&self) -> f64 {
        if let Some(missed) = self.tree.missed_share(LEAST_LOOKUPS) {
            return 1.0_f64 - missed;
        }
        let count = |n: usize| f64::from(u32::try_from(n).unwrap_or(u32::MAX));
        let page = count(self.pool.capacity(self.file).unwrap_or(4096));
        let pool = count(self.pool.frames()) * page;
        let width = self.gathered.get().map_or(page / 100.0_f64, |s| s.width.max(1.0_f64));
        let table = count(self.counted.live.load(Ordering::Acquire)) * (width + ENTRY_BYTES);
        if table > 0.0_f64 { (pool / table).min(1.0_f64) } else { 1.0_f64 }
    }

    /// The share of the pages the pool misses the system's cache holds: of
    /// the file's pages in use past the pool's, as many as
    /// STORAGE.SYSTEM_CACHE_BYTES; all if none are past it.
    fn cached_share(&self) -> f64 {
        let pages = |n: u64| crate::stats::parts::count(n);
        let page = self.pool.capacity(self.file).ok().and_then(|c| u64::try_from(c).ok()).unwrap_or(4096).max(1);
        let cache = pages(self.system_cache.load(Ordering::Acquire).checked_div(page).unwrap_or(u64::MAX));
        let frames = pages(u64::try_from(self.pool.frames()).unwrap_or(u64::MAX));
        let past = pages(self.pool.pages_in_use(self.file).unwrap_or(0)) - frames;
        if past > 0.0_f64 { (cache / past).min(1.0_f64) } else { 1.0_f64 }
    }

    /// The planner's cost of a row fetched by id (`TableStorage::fetch_cost`):
    /// `ROW_FETCH_HELD` of the share the pool holds; of the rest, as read
    /// from the system's cache by the share it holds, else from the disk;
    /// of the share of the rows frozen, `FROZEN_FETCH` and
    /// `FROZEN_FETCH_COLUMN` for each of the `columns` read of that.
    pub(super) fn frozen_fetch_cost(&self, columns: usize) -> f64 {
        let (frozen, all) = self.frozen_share();
        let share = if all > 0.0_f64 { (frozen / all).min(1.0_f64) } else { 0.0_f64 };
        let fetch = ROW_FETCH_HELD + self.page_missed_cost() * (1.0_f64 - self.held_share());
        let frozen_row = FROZEN_FETCH + FROZEN_FETCH_COLUMN * crate::storage::cost_of(columns);
        fetch * (1.0_f64 - share + frozen_row * share)
    }

    /// The cost more of a fetch whose page the pool missed than of one it
    /// held (`TableStorage::miss_cost`): read from the system's cache by
    /// the share it holds, else from the disk.
    pub(super) fn page_missed_cost(&self) -> f64 {
        ROW_FETCH_DISK + (ROW_FETCH_CACHED - ROW_FETCH_DISK) * self.cached_share() - ROW_FETCH_HELD
    }

    /// The rows frozen, and all the table's rows, as costs.
    fn frozen_share(&self) -> (f64, f64) {
        let frozen = self.frozen.directory().map_or(0, |d| d.rows());
        (
            crate::storage::cost_of(usize::try_from(frozen).unwrap_or(usize::MAX)),
            crate::storage::cost_of(self.counted.live.load(Ordering::Acquire)),
        )
    }

    /// Notes a commit at `at`, for COLUMNSTORE.FREEZE_DELAY.
    pub(super) fn note_commit(&self, at: Timestamp) {
        if self.frozen.plain() {
            return;
        }
        let Some(columns) = read(&self.frozen.columns).ok().and_then(|c| *c) else { return };
        if columns.freeze_delay > 0 {
            freeze::noted(&mut lock_builtin(&self.frozen.commits), at, columns.freeze_delay);
        }
    }
}

// Unit tests: rows frozen when due, under readers and as rows are deleted;
// frozen rows written, recovered and reopened; groups dropped, written
// again, compressed and sorted; headers read as they are needed (RULES.md
// R-TEST-1).
#[cfg(test)]
mod tests;

// Unit tests: frozen rows read past the tree's entries shadowing them.
#[cfg(test)]
mod shadowed;
