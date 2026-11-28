//! \[DESIGN\] The row-store layout of the built-in engine (RULES.md D31): a
//! table is a B-link tree (`tree.rs`) keyed by row id, each entry the
//! newest version of a row (`rows/version.rs`) holding its record
//! (`record.rs`), in one pager file shared by the engine's tables through
//! the buffer pool. Older versions are in the undo store (`rows/undo.rs`).
//!
//! * **Row ids** are logical, 64-bit, given in blocks, one per inserting
//!   thread, in increasing order within each (`rows/ids.rs`), so a thread
//!   alone fills the tree's pages in order and keeps them full, and threads
//!   at once fill different leaves; they never change with updates or
//!   splits, and are never given out again. The table's meta page records
//!   the ids reserved, so a table opened again goes on past every id it
//!   gave.
//! * **Long values** go out of line (`record.rs`): a value of varying width
//!   longer than the table's threshold (by default a quarter of a page's
//!   capacity), and the longest others if the row would not fit its leaf,
//!   each in a page of its own, of any length (the pager's overflow). The
//!   page begins with the row id and column it belongs to, checked on every
//!   read, so a reference that has gone stale is refused, never followed
//!   to another row's value. Each version has pages of its own; they are
//!   freed with it, when it is garbage, or when the change that made it is
//!   undone. One that cannot be freed is left, never failing a change
//!   already made.
//! * **Encodings** the table's option ENCODINGS names, such as a
//!   compression, seal its records and long values (`rows/sealing.rs`).
//! * **Transactions** are isolated by the engine with versions
//!   (`rows/mvcc.rs`), serializably, or below SERIALIZABLE as each is told;
//!   it undoes its own changes.
//! * **Scans** decode only the columns asked for, and fetch only their
//!   values out of line. A table of the row store skips the blocks of row
//!   ids its zone maps say a scan's conditions cannot match
//!   (`rows/zones.rs`); one of the column store freezes its rows into
//!   groups of their columns' values (`rows/frozen.rs`).
//!
//! Durability is the log's (D25): the table is `Logged`, durable through
//! the store's last checkpoint (`StorageEngine::checkpoint`), taken when
//! asked, when one comes due, and when the database is closed
//! (`db/checkpoint.rs`); its locator, the meta page and that position,
//! finds it again in its file (`StorageEngine::open_table`).

mod count;
mod counted;
mod discard;
mod engine;
mod files;
mod framed;
mod frozen;
mod gathered;
mod ids;
mod in_place;
mod lobs;
mod meta;
mod mvcc;
mod options;
mod recovery;
mod scan;
mod sealing;
mod sizes;
mod split;
mod stats;
mod store;
mod table;
mod undo;
mod values;
mod version;
mod zones;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

pub use files::TableFiles;
use frozen::Frozen;
pub use frozen::{SYSTEM_CACHE_BYTES, extents::SegmentPlace};
use ids::IdBlocks;
use ids::{key, no_row, row_id};
use lobs::LobSpace;
pub use meta::TABLE_FORMAT;
use meta::{locator_of, meta_content};
use mvcc::{Past, Read, Versions, columns_mask};
pub use options::{DEFAULT_HELD_BYTES, DEFAULT_REFRESH_FRACTION, DEFAULT_REFRESH_ROWS, DEFAULT_TRACKED_ROWS, Refresh, TableOptions};
use sealing::Sealing;
use stats::Gathered;
pub use store::RowStore;
pub use undo::UndoStore;
use zones::{Replaced, Zones};

use super::columns::cache;
use super::record::{RecordFormat, Spill};
use super::tree::BLinkTree;
use crate::catalog::TableDesc;
use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::pager::PageId;
use crate::error::{Code, Result, SqlError};
use crate::log::Lsn;
use crate::stats::Statistics;
use crate::storage::VettedBatches;
use crate::storage::{
    Batches, Condition, Conjunction, Garbage, Near, Newest, Region, Removed, Row, RowId, Rows, ScanOrder, Split, TableStorage,
};
use crate::txn::{Concurrency, NO_TRANSACTION, Persistence, ReadMode, Timestamp, TxCapabilities, TxId, Undo};

/// The name of the row store.
pub const ROW_STORE: &str = "rowstore";
/// \[DESIGN\] The name of the column store: the row store's tables, each
/// freezing its rows into groups of its columns' values (RULES.md D31).
pub const COLUMN_STORE: &str = "columnstore";
/// Row ids reserved at a time: the meta page is written once per block.
const RESERVE: u64 = 1024;
/// Bytes before a long value in its page: its row id and column.
const OWNER: usize = 12;
/// Bytes of a row id as a key.
const KEY: usize = 8;
/// The most bytes of a version before its record (`rows/version.rs`).
const VERSION: usize = 17;

fn corrupt(why: &str) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[DESIGN] a row-store table is malformed: {why}"))
}

/// \[DESIGN\] A table of the row store.
pub struct RowTable {
    pool: Arc<BufferPool>,
    file: FileId,
    undo: Arc<UndoStore>,
    versions: Versions,
    meta: PageId,
    tree: BLinkTree,
    format: RecordFormat,
    spill: Spill,
    /// The first row id of the next block to give (`rows/ids.rs`).
    next: Arc<AtomicU64>,
    /// The threads' blocks of row ids.
    blocks: IdBlocks,
    /// Row ids below this may have been given, as the meta page records.
    reserved: AtomicU64,
    /// Held while the reservation is moved and the meta page written.
    reserving: Mutex<()>,
    /// Its count of rows, saved at each checkpoint (`rows/counted.rs`).
    counted: Arc<counted::Counted>,
    /// Its statistics (`rows/stats.rs`).
    gathered: Gathered,
    /// The store's durable position (`TableFiles`).
    durable: Arc<AtomicU64>,
    /// Set when the table is dropped: its pages are freed when it goes (`rows/discard.rs`).
    discarded: AtomicBool,
    /// How its rows are sealed (`rows/sealing.rs`), its zone maps too.
    sealing: Arc<Sealing>,
    /// The kernels its frozen groups are decoded and judged on (RULES.md D570): its store's as it was opened.
    kernels: Arc<dyn crate::kernels::Kernels>,
    /// Its frozen row groups, of the column store (`rows/frozen.rs`).
    frozen: Frozen,
    /// Held shared while it freezes rows (`TableFiles::freezes`).
    freezes: Arc<RwLock<()>>,
    /// Its zone maps, of the row store (`rows/zones.rs`), saved at each checkpoint.
    zones: Arc<Zones>,
    /// Its store's headers of frozen row groups held (`TableFiles::headers`).
    headers: Arc<cache::HeaderCache>,
    /// The bytes of its file the system's cache is taken to hold beyond the
    /// pool (`TableFiles::system_cache`).
    system_cache: Arc<AtomicU64>,
    /// Its large objects (`rows/lobs.rs`), shared with the values read from it.
    lobs: Arc<LobSpace>,
}

impl TableStorage for RowTable {
    fn insert(&self, tx: TxId, row: Row) -> Result<RowId> {
        self.insert_from(tx, &row)
    }

    fn loading(&self) -> Option<&dyn crate::storage::Loading> {
        Some(self)
    }

    /// Encoded as it is held, never copied.
    fn insert_from(&self, tx: TxId, row: &Row) -> Result<RowId> {
        // Past the groups of a load, so its rows go to the tree from now on.
        self.end_direct(tx);
        let id = self.next_id()?;
        let (mut framed, pages) = self.encode_framed(id, row)?;
        match self.insert_version(tx, id, &mut framed) {
            Ok(true) => {
                // Widened once written, so a zone summed meanwhile either
                // read the row or is summed again (`rows/zones.rs`).
                self.zones.widen(id, Some(row), Replaced::Nothing);
                self.counted.live.fetch_add(1, Ordering::AcqRel);
                self.counted_change(tx);
                Ok(id)
            }
            Ok(false) => {
                self.free(&pages);
                Err(corrupt(&format!("row id {id}, never given, is in the table")))
            }
            Err(e) => {
                self.free(&pages);
                Err(e)
            }
        }
    }

    /// Its tree's (`BLinkTree::pin`).
    fn pin(&self) -> Option<crate::engine::tree::Pinned<'_>> {
        Some(self.tree.pin())
    }

    fn update(&self, tx: TxId, id: RowId, row: Row) -> Result<()> {
        self.write_unread(tx, id, Some(&row), Past::Snapshot)
    }

    fn delete(&self, tx: TxId, id: RowId) -> Result<()> {
        self.write_unread(tx, id, None, Past::Snapshot)
    }

    fn replace(&self, tx: TxId, id: RowId, row: Option<&Row>) -> Result<Row> {
        if tx == NO_TRANSACTION {
            let old = self.get(tx, id)?.map_or_else(|| no_row(id), Ok)?;
            return self.write_over(tx, id, row, Past::Snapshot).map(|_| old);
        }
        self.write_over(tx, id, row, Past::Snapshot)?.map_or_else(|| no_row(id), Ok)
    }

    /// The row replaced with its columns read alone decoded, its record patched if told the columns set (D875, D916).
    fn replace_set(&self, tx: TxId, id: RowId, row: Option<&Row>, told: (Option<crate::storage::Patch<'_>>, &[usize])) -> Result<Row> {
        self.replace_reading(tx, id, row, told)
    }

    fn write_newest(&self, tx: TxId, id: RowId, row: Option<&Row>, seen: Timestamp) -> Result<()> {
        self.write_unread(tx, id, row, Past::Stamp(seen))
    }

    fn keep_entries(&self, tx: TxId, id: RowId, unchanged: u64) -> u64 {
        self.keep_columns(tx, id, unchanged)
    }

    /// Puts row `id` back, or replaces it if it is there; ids given from
    /// now on are past it.
    fn restore(&self, tx: TxId, id: RowId, row: Row) -> Result<()> {
        self.next.fetch_max(id.saturating_add(1), Ordering::SeqCst);
        // A thread's block may hold the id: blocks are taken past it now.
        self.blocks.clear();
        self.reserve_past(id)?;
        let (mut framed, pages) = self.encode_framed(id, &row)?;
        let restored = match self.insert_version(tx, id, &mut framed) {
            Ok(true) => {
                self.counted.live.fetch_add(1, Ordering::AcqRel);
                Ok(())
            }
            Ok(false) => self.write_version(tx, id, Some(&mut framed), Past::Snapshot).map(|_| ()),
            Err(e) => Err(e),
        };
        restored.inspect_err(|_| self.free(&pages))?;
        self.zones.widen(id, Some(&row), Replaced::Unknown);
        Ok(())
    }

    fn get(&self, tx: TxId, id: RowId) -> Result<Option<Row>> {
        self.get_of(tx, id, None, &mut Near::default())
    }

    /// Only `columns` decoded; of a frozen row, only their chunks read.
    fn get_columns(&self, tx: TxId, id: RowId, columns: &[usize]) -> Result<Option<Row>> {
        self.get_of(tx, id, Some(columns), &mut Near::default())
    }

    /// Reads the row from the leaf of the last read of `near`, if it can.
    fn get_near(&self, tx: TxId, id: RowId, columns: Option<&[usize]>, near: &mut Near) -> Result<Option<Row>> {
        self.get_of(tx, id, columns, near)
    }

    /// Only `columns` decoded, into the row held (`rows/scan.rs`).
    fn get_listed(&self, tx: TxId, id: RowId, columns: &[usize], held: (&mut Near, &mut Row)) -> Result<bool> {
        RowTable::get_listed(self, tx, id, columns, held)
    }

    /// Only `columns` are decoded, and only their values out of line read.
    fn scan(self: Arc<Self>, tx: TxId, columns: &[usize]) -> Rows {
        self.scan_rows(tx, columns, None)
    }

    /// As `scan`, leaving out the rows `condition` does not match, which do not count as read (`scan_rows`). Of a table that
    /// holds no group, only the rows of the zones it cannot match are left out.
    fn scan_where(self: Arc<Self>, tx: TxId, columns: &[usize], condition: &Conjunction) -> Rows {
        self.scan_rows(tx, columns, Some(condition.clone()))
    }

    /// A table of the column store gives its frozen rows without making
    /// each (`rows/frozen/batches.rs`); its tree's rows, and a table of the
    /// row store its rows, each record decoded straight into the vectors
    /// (`rows/gathered.rs`); its frozen rows' columns of `late` as their
    /// places (`rows/frozen/late.rs`).
    fn scan_batches(self: Arc<Self>, tx: TxId, columns: &[usize], condition: Option<&Conjunction>, late: &[usize]) -> Batches {
        self.scan_batches_of(tx, columns, condition.cloned(), (late, &[]))
    }

    /// \[DESIGN\] (RULES.md D1266, D1267) As `scan_batches`, its frozen rows' columns of `asked.1` measured, late with their octet lengths
    /// (`rows/frozen/batches/measured.rs`), each batch of the column store's frozen rows with what the scan judged of it (`rows/scan.rs`).
    fn scan_vetted(self: Arc<Self>, tx: TxId, what: (&[usize], Option<&Conjunction>), asked: (&[usize], &[usize])) -> VettedBatches {
        self.scan_vetted_of(tx, what.0, what.1.cloned(), asked)
    }

    /// Split at row ids, of the column store at its groups' starts
    /// (`rows/split.rs`).
    fn split(self: Arc<Self>, tx: TxId, columns: &[usize], conditions: Option<&Conjunction>, rows: usize) -> Option<Arc<dyn Split>> {
        let split: Arc<dyn Split> = Arc::new(self.split_of(tx, columns, conditions.cloned(), rows).ok()?);
        Some(split)
    }

    /// Every table does: of the column store, its frozen rows straight
    /// from their chunks; of either, its tree's records decoded straight
    /// into the vectors (`rows/gathered.rs`), no row made.
    fn reads_by_batch(&self) -> bool {
        true
    }

    /// A table of the column store does.
    fn keeps_columns(&self) -> bool {
        self.frozen.columns().is_some()
    }

    /// Of the column store, the columns COLUMNSTORE.ORDER names.
    fn ordered_by(&self) -> Vec<usize> {
        self.frozen.columns().map(|c| c.order.places()).unwrap_or_default()
    }

    /// Of the column store, its groups' rows and the tree's merged in the
    /// order of `keys`, or backward, of the first `most` alone if only those
    /// are asked for (`rows/frozen/ordered.rs`).
    fn scan_ordered(self: Arc<Self>, tx: TxId, columns: &[usize], order: ScanOrder<'_>, conditions: Option<&Conjunction>) -> Option<Rows> {
        (!order.0.is_empty() && self.ordered_by().starts_with(order.0))
            .then(|| self.scan_ordered_of(tx, (columns, order), conditions.cloned()))
    }

    /// Of the column store, merged by batch, of `columns` alone, no row
    /// made (`rows/frozen/ordered.rs`).
    fn scan_ordered_batches(
        self: Arc<Self>,
        tx: TxId,
        columns: &[usize],
        order: ScanOrder<'_>,
        conditions: Option<&Conjunction>,
    ) -> Option<Batches> {
        (!order.0.is_empty() && self.ordered_by().starts_with(order.0))
            .then(|| self.scan_ordered_batches_of(tx, (columns, order), conditions.cloned()))
    }

    /// From its zones where settled, or of one that holds groups from their directory (`rows/count.rs`).
    fn count(self: Arc<Self>, tx: TxId) -> Option<crate::storage::Counts> {
        Some(self.count_of(tx))
    }

    /// Of one that holds groups, each group no entry shadows whole (`rows/frozen/summarized.rs`).
    fn summarized(self: Arc<Self>, tx: TxId, columns: &[usize]) -> Option<crate::storage::Pieces> {
        self.summarized_of(tx, columns)
    }

    fn row_count(&self) -> usize {
        self.counted.live.load(Ordering::Acquire)
    }

    fn sizes(&self, by_column: bool) -> Option<crate::storage::TableSizes> {
        self.measured(by_column)
    }

    /// A row of the tree costs 0.8, up to 1 as all its columns are read; a
    /// frozen row 0.25, up to 1 (`rows/frozen.rs`).
    fn scan_cost(&self, columns: &[usize]) -> f64 {
        self.frozen_scan_cost(columns)
    }

    /// Of the zones (`rows/zones.rs`), or the frozen chunks, its conditions
    /// may match.
    fn scan_share(&self, conditions: &Conjunction) -> f64 {
        if self.frozen.columns().is_some() { self.frozen_read_share(conditions) } else { self.zones.share(conditions) }
    }

    /// 8.6 of a table the buffer pool holds, more as it holds less of it, shared with the other tables and indexes of its file,
    /// to 72, to 5900 as the system's cache holds none of the rest; more by the share of the rows frozen (`rows/frozen.rs`).
    fn fetch_cost(&self, columns: usize) -> f64 {
        self.frozen_fetch_cost(columns)
    }

    /// 63.4 while its file's pages past the pool fit the system's cache, more as they do not, to 5891.4 (`rows/frozen.rs`).
    fn miss_cost(&self) -> f64 {
        self.page_missed_cost()
    }

    /// A table opened has those a checkpoint saved, else none until the plan first asking gathers them (RULES.md D1092, D1093).
    fn statistics(&self) -> Option<Arc<dyn Statistics>> {
        self.statistics_asked()
    }

    fn statistics_passes(&self) -> u64 {
        self.gathered.passes()
    }

    fn statistics_wanted(&self) -> bool {
        self.gather_wanted()
    }

    fn garbage(self: Arc<Self>, as_of: Timestamp) -> Garbage {
        self.pending(Box::new(move |at, _| at <= as_of), None)
    }

    /// Writes the row's newest version outright (`rows/recovery.rs`).
    fn apply(&self, id: RowId, row: Option<Row>) -> Result<()> {
        self.apply_newest(id, row.as_ref())?;
        self.zones.widen(id, row.as_ref(), Replaced::Unknown);
        Ok(())
    }

    fn analyze(&self) -> Result<()> {
        self.gather().map(|_| ())
    }

    /// The position of the store's last checkpoint.
    fn durable_through(&self) -> Lsn {
        Lsn(self.durable.load(Ordering::Acquire))
    }

    fn locator(&self) -> Vec<u8> {
        locator_of(self.meta, self.durable.load(Ordering::Acquire))
    }

    fn discard(&self) {
        self.discarded.store(true, Ordering::Release);
        self.lobs.discard();
    }
    fn reconfigure(&self, desc: &TableDesc) {
        self.frozen.reconfigure(options::columns_of(desc, self.frozen.columns()));
        // Of the column store from now on, it reads by zone (`zones/cold.rs`).
        if self.frozen.columns().is_some() {
            self.zones.warm();
        }
        self.sealing.rename(&options::encodings_of(desc));
        self.gathered.reconfigure(desc);
        self.versions.reconfigure(desc);
    }

    fn capabilities(&self) -> TxCapabilities {
        TxCapabilities { undo: Undo::Native, concurrency: Concurrency::Engine { serializable: true }, persistence: Persistence::Logged }
    }

    fn begin(&self, tx: TxId, snapshot: Timestamp) -> Result<()> {
        self.begin_open(tx, snapshot);
        Ok(())
    }

    fn read_mode(&self, tx: TxId, mode: ReadMode) -> Result<()> {
        self.read_mode_of(tx, mode)
    }

    fn preserving(&self, tx: TxId) -> Result<()> {
        self.preserving_of(tx)
    }

    /// Of its zone, as the maintenance thread settles it (`rows/zones.rs`).
    fn visible_to_all(&self, id: RowId) -> bool {
        self.zones.visible(id)
    }

    /// \[DESIGN\] (RULES.md D1233) All of them: each zone is settled
    /// visible once collection and the horizon pass its writes. Asked by a
    /// plan weighing an index-only read, which warms the table (D868).
    fn settled_share(&self) -> Option<f64> {
        self.zones.warm();
        Some(1.0_f64)
    }

    /// Its zone's count of versions superseded one less.
    fn taken_back(&self, id: RowId) {
        self.zones.superseded(id, false);
    }

    fn savepoint(&self, tx: TxId) -> u64 {
        u64::try_from(self.changes(tx)).unwrap_or(u64::MAX)
    }

    fn rollback_to(&self, tx: TxId, savepoint: u64) -> Result<()> {
        self.undo_changes(tx, usize::try_from(savepoint).unwrap_or(usize::MAX))
    }

    fn validate(&self, tx: TxId, at: Timestamp) -> Result<()> {
        self.validate_open(tx, at)
    }

    fn commit(&self, tx: TxId, at: Timestamp) {
        self.commit_open(tx, at);
        self.stamp_groups(tx, at);
        self.note_commit(at);
    }

    fn settle(&self, tx: TxId) {
        self.settle_open(tx);
    }

    fn rollback(&self, tx: TxId) -> Result<()> {
        self.end_direct(tx);
        self.end_open(tx)
    }

    /// Copies each large object of `row` it does not keep for that row
    /// (`rows/lobs/owned.rs`).
    fn keep_lobs(&self, tx: TxId, replacing: Option<RowId>, row: &mut Row, log: crate::storage::LobLog<'_>) -> Result<()> {
        RowTable::keep_lobs(self, tx, replacing, row, log)
    }

    fn apply_lob(&self, id: u64, body: &[u8]) -> Result<()> {
        self.apply_object(id, body)
    }

    fn note_stale_lobs(&self, ids: &[u64]) {
        self.lobs.note_candidates(ids);
    }

    fn recovered(&self) -> Result<()> {
        self.sweep_objects()
    }

    fn stale_lobs(&self) -> Result<Vec<u64>> {
        self.pending_objects()
    }

    fn read_by_condition(&self, tx: TxId, condition: &Condition) -> Result<()> {
        // RULES.md D448: a covering scan consumes the index's columns; any other reads each row it chose by id, with its columns.
        self.note_read(tx, || Read::Condition(Conjunction::of(condition.clone()), columns_mask(condition.columns.iter().copied())))
    }

    fn read_by_region(&self, tx: TxId, region: &Region) -> Result<()> {
        self.note_read(tx, || Read::Region(region.clone()))
    }

    fn newest(&self, tx: TxId, id: RowId) -> Result<Newest> {
        self.newest_of(tx, id)
    }

    fn newest_writer(&self, tx: TxId, id: RowId) -> Result<(Option<TxId>, Timestamp)> {
        self.writer_of(tx, id)
    }

    /// Statistics due are the maintenance's (`maintenance_due`).
    fn uncollected(&self) -> bool {
        self.awaits_collection()
    }

    fn collection_due(&self, horizon: Timestamp) -> bool {
        self.due_at(horizon)
    }

    fn gc(&self, horizon: Timestamp, removed: Removed<'_>) -> Result<()> {
        self.collect(horizon, removed)
    }

    /// Statistics to gather; zones to sum up; rows to freeze, groups to write again, entries to take out.
    fn maintenance_due(&self) -> bool {
        self.gather_wanted() || self.zones.wanted() || self.freeze_wanted() || self.groups_pending()
    }

    /// Statistics gathered as one call's work, once spaced from the last
    /// pass or `now` (`rows/stats.rs`), zones summed up as the next's,
    /// freezing as the next's: freezing as this one's too if no zone could
    /// be summed up, as one of a commit too recent waits for the horizon
    /// (`rows/zones.rs`). Zones resting, or waiting, are not summed up as
    /// another table's work or an index's calls this, unless `now` (RULES.md D772).
    fn maintain(&self, horizon: Timestamp, now: bool) -> Result<bool> {
        if self.gather_if_due(now) {
            return Ok(true);
        }
        let summing = if now { self.zones.due() } else { self.zones.wanted() };
        if summing && self.sum_zones(horizon, now)? {
            return Ok(true);
        }
        self.settle_groups(horizon)?;
        self.freeze_due(horizon)
    }

    fn versions(self: Arc<Self>, horizon: Timestamp, as_of: Timestamp, columns: &[usize]) -> Rows {
        self.versions_as_of(horizon, as_of, columns)
    }
}

// Unit tests: a long value's page is checked against its row and column,
// a meta page against its format, an undone change keeps its older
// version for open readers, deleted rows are not written, versions and
// garbage are read as of a time, and a dropped table frees its pages (RULES.md R-TEST-1, R-TEST-2, R-TEST-7).
#[cfg(test)]
mod tests;
