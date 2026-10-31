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

mod counted;
mod discard;
mod files;
mod frozen;
mod gathered;
mod ids;
mod meta;
mod mvcc;
mod options;
mod recovery;
mod scan;
mod sealing;
mod stats;
mod store;
mod table;
mod undo;
mod values;
mod version;
mod zones;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

pub use files::TableFiles;
pub use frozen::SYSTEM_CACHE_BYTES;
use frozen::{Frozen, Merged, Merging};
use ids::IdBlocks;
use ids::{key, no_row, row_id};
pub use meta::TABLE_FORMAT;
use meta::{locator_of, meta_content, parse_locator};
use mvcc::{Read, Versions};
pub use options::{DEFAULT_HELD_BYTES, DEFAULT_TRACKED_ROWS, Refresh, TableOptions};
use sealing::Sealing;
use stats::Gathered;
pub use undo::UndoStore;
use zones::Zones;

use super::columns::cache;
use super::record::{RecordFormat, Spill};
use super::tree::BLinkTree;
use crate::catalog::{QualifiedName, TableDesc};
use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::encoding::Encodings;
use crate::disk::io::Durability;
use crate::disk::pager::PageId;
use crate::error::{Code, Result, SqlError};
use crate::log::Lsn;
use crate::memory::Host;
use crate::settings::Configurable;
use crate::spill::WorkAreas;
use crate::stats::Statistics;
use crate::storage::{Condition, Conjunction, Newest, Row, RowId, Rows, StorageEngine, TableStorage};
use crate::sync::{lock_builtin, write};
use crate::txn::{Concurrency, Persistence, ReadMode, Timestamp, TxCapabilities, TxId, Undo};
use crate::value::DataType;

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

/// \[DESIGN\] The row store: tables as B-link trees of row versions, in
/// one pager file of a buffer pool, older versions in an undo store.
pub struct RowStore {
    /// Where its tables are; for a store in memory, made when the first
    /// table is.
    files: Mutex<Option<TableFiles>>,
    /// Its pages in memory, for files made in memory: of a budget of its
    /// own until it is attached to a database's (`attach`).
    memory: Arc<crate::engine::MemoryPages>,
    defaults: TableOptions,
    /// Options for tables not yet created, by name.
    tables: Mutex<HashMap<QualifiedName, TableOptions>>,
    /// The database's work areas, once attached, shared with its tables.
    work: Arc<RwLock<Option<Arc<WorkAreas>>>>,
    /// The database's encodings, once attached: those its tables' option
    /// ENCODINGS names.
    encodings: Mutex<Arc<Encodings>>,
    /// Its name: `ROW_STORE`, or `COLUMN_STORE`, whose tables freeze their
    /// rows.
    name: &'static str,
    /// Whether it checkpoints its files: not the column store sharing the
    /// row store's, which checkpoints them for both.
    checkpoints: bool,
    /// The headers of its tables' frozen row groups held in memory, of
    /// the database's budget once attached (`columns/cache.rs`).
    headers: Arc<cache::HeaderCache>,
    /// STORAGE.SYSTEM_CACHE_BYTES, as its tables over files on a disk
    /// price their fetches by it; none for a store in memory, whose files
    /// are all held.
    system_cache: Option<Arc<AtomicU64>>,
}

impl StorageEngine for RowStore {
    fn name(&self) -> &'static str {
        self.name
    }
    fn seals(&self) -> bool {
        true
    }
    fn attach(&self, host: &Host) -> Result<()> {
        self.use_budget(Arc::clone(&host.memory));
        self.use_work(Arc::clone(&host.work));
        self.use_encodings(Arc::clone(&host.encodings));
        Ok(())
    }
    fn table_options(&self) -> Vec<crate::storage::TableOption> {
        options::declared(self.defaults)
    }
    /// It reads its settings (`Configurable`).
    fn configurable(&self) -> Option<&dyn Configurable> {
        Some(self)
    }
    fn create_table(&self, desc: &TableDesc) -> Result<Box<dyn TableStorage>> {
        let options = options::resolved(desc, lock_builtin(&self.tables).get(&desc.name).copied().unwrap_or(self.defaults));
        let types: Vec<DataType> = desc.columns.iter().map(|c| c.data_type).collect();
        Ok(Box::new(RowTable::create(&self.files()?, &types, options)?.sealed(self.sealing(desc))))
    }

    /// The table whose meta page the locator names; its pages hold every
    /// change through the position the locator records (`locator`).
    fn open_table(&self, desc: &TableDesc, locator: &[u8]) -> Result<Option<Box<dyn TableStorage>>> {
        let (meta, durable) = parse_locator(locator).ok_or_else(|| corrupt("a table's locator is not the row store's"))?;
        let options = options::resolved(desc, lock_builtin(&self.tables).get(&desc.name).copied().unwrap_or(self.defaults));
        let types: Vec<DataType> = desc.columns.iter().map(|c| c.data_type).collect();
        let files = self.files()?;
        files.durable.fetch_max(durable, Ordering::AcqRel);
        Ok(Some(Box::new(RowTable::open_with(&files, (meta, &types), options, self.sealing(desc))?)))
    }

    /// Saves its tables' zone maps and counts of rows
    /// (`rows/zones/saved.rs`, `rows/counted.rs`); writes back every page and the pager's free list; a file shadowed
    /// (`TableFiles::shadowed`) holds them in memory and seals that batch,
    /// its commit left to `checkpoint_finish`; one not shadowed is synced
    /// now.
    fn checkpoint(&self, at: Lsn) -> Result<()> {
        if !self.checkpoints {
            return Ok(());
        }
        let files = self.files()?;
        // No table's rows half frozen (`rows/frozen.rs`).
        let _frozen = write(&files.freezes)?;
        if let Some(shadows) = &files.shadows {
            // What is written now is kept in memory: no I/O with writers held.
            shadows.hold()?;
            let captured =
                files.save_zones().and_then(|()| files.save_counts()).and_then(|()| files.pool.sync(files.file, Durability::Full));
            if let Err(e) = captured.and_then(|()| shadows.seal()) {
                let _ = shadows.release();
                return Err(e);
            }
            files.sealed.store(at.0, Ordering::Release);
            return Ok(());
        }
        files.save_zones()?;
        files.save_counts()?;
        files.pool.sync(files.file, Durability::Full)?;
        files.durable.fetch_max(at.0, Ordering::AcqRel);
        Ok(())
    }

    /// Commits the batch sealed, so after a crash the file holds exactly
    /// what the checkpoint captured.
    fn checkpoint_finish(&self) -> Result<()> {
        if !self.checkpoints {
            return Ok(());
        }
        let files = self.files()?;
        if let Some(shadows) = &files.shadows {
            shadows.commit_sealed()?;
            files.durable.fetch_max(files.sealed.load(Ordering::Acquire), Ordering::AcqRel);
        }
        Ok(())
    }
}

impl Configurable for RowStore {
    /// The column store's, whose tables freeze rows into groups: the
    /// bytes of their headers it holds (`columns/cache.rs`).
    fn settings(&self) -> Vec<crate::settings::Definition> {
        (self.name == COLUMN_STORE).then(|| cache::declared(self.name)).into_iter().collect()
    }
    /// The headers it holds; the pages its pool reads ahead, of its data
    /// file's or in memory (STORAGE.READ_AHEAD_PAGES), which index methods
    /// on the same pool share; and, over files on a disk, the bytes of
    /// them the system's cache holds (STORAGE.SYSTEM_CACHE_BYTES).
    fn configure(&self, settings: &crate::settings::Settings) {
        if let Some(key) = settings.key(&cache::setting_name(self.name)) {
            self.headers.set_most(settings.count(key));
        }
        self.set_read_ahead(settings.count(crate::settings::READ_AHEAD_PAGES));
        if let Some(cache) = &self.system_cache {
            cache.store(u64::try_from(settings.count(crate::settings::SYSTEM_CACHE_BYTES)).unwrap_or(u64::MAX), Ordering::Release);
        }
    }
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
    next: AtomicU64,
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
    /// Set when the table is dropped: its pages are freed when it goes
    /// (`rows/discard.rs`).
    discarded: AtomicBool,
    /// How its rows are sealed (`rows/sealing.rs`), its zone maps too.
    sealing: Arc<Sealing>,
    /// Its frozen row groups, of the column store (`rows/frozen.rs`).
    frozen: Frozen,
    /// Held shared while it freezes rows (`TableFiles::freezes`).
    freezes: Arc<RwLock<()>>,
    /// Its zone maps, of the row store (`rows/zones.rs`), saved at each
    /// checkpoint.
    zones: Arc<Zones>,
    /// Its store's headers of frozen row groups held (`TableFiles::headers`).
    headers: Arc<cache::HeaderCache>,
    /// The bytes of its file the system's cache is taken to hold beyond the
    /// pool (`TableFiles::system_cache`).
    system_cache: Arc<AtomicU64>,
}

impl TableStorage for RowTable {
    fn insert(&self, tx: TxId, row: Row) -> Result<RowId> {
        let id = self.next_id()?;
        let (record, pages) = self.encode(id, &row)?;
        match self.insert_version(tx, id, &record) {
            Ok(true) => {
                // Widened once written, so a zone summed meanwhile either
                // read the row or is summed again (`rows/zones.rs`).
                self.zones.widen(id, Some(&row), false);
                self.counted.live.fetch_add(1, Ordering::AcqRel);
                self.gathered.changed();
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

    fn update(&self, tx: TxId, id: RowId, row: Row) -> Result<()> {
        let (record, pages) = self.encode(id, &row)?;
        self.write_version(tx, id, Some(&record)).inspect_err(|_| self.free(&pages))?;
        self.zones.widen(id, Some(&row), true);
        self.gathered.changed();
        Ok(())
    }

    fn delete(&self, tx: TxId, id: RowId) -> Result<()> {
        self.write_version(tx, id, None)?;
        self.zones.widen(id, None, true);
        self.counted.live.fetch_sub(1, Ordering::AcqRel);
        self.gathered.changed();
        Ok(())
    }

    /// Puts row `id` back, or replaces it if it is there; ids given from
    /// now on are past it.
    fn restore(&self, tx: TxId, id: RowId, row: Row) -> Result<()> {
        self.next.fetch_max(id.saturating_add(1), Ordering::AcqRel);
        // A thread's block may hold the id: blocks are taken past it now.
        self.blocks.clear();
        self.reserve_past(id)?;
        let (record, pages) = self.encode(id, &row)?;
        let restored = match self.insert_version(tx, id, &record) {
            Ok(true) => {
                self.counted.live.fetch_add(1, Ordering::AcqRel);
                Ok(())
            }
            Ok(false) => self.write_version(tx, id, Some(&record)),
            Err(e) => Err(e),
        };
        restored.inspect_err(|_| self.free(&pages))?;
        self.zones.widen(id, Some(&row), true);
        Ok(())
    }

    fn get(&self, tx: TxId, id: RowId) -> Result<Option<Row>> {
        let reader = self.reader(tx)?;
        self.note_read(tx, Read::Row(id))?;
        match self.tree.get(&key(id))? {
            Some(newest) => self.visible_from(newest, reader)?.map(|record| self.decode(id, &record, None)).transpose(),
            // Frozen, so committed before every snapshot (`rows/frozen.rs`).
            None => self.frozen_row(id, None),
        }
    }

    /// Only `columns` are decoded, and only their values out of line read.
    fn scan(self: Arc<Self>, tx: TxId, columns: &[usize]) -> Rows {
        self.scan_rows(tx, columns, None)
    }

    /// As `scan`, leaving out the rows `condition` does not match; only
    /// those it matches count as read.
    fn scan_where(self: Arc<Self>, tx: TxId, columns: &[usize], condition: &Conjunction) -> Rows {
        self.scan_rows(tx, columns, Some(condition.clone()))
    }

    /// A table of the column store gives its frozen rows without making
    /// each (`rows/frozen/batches.rs`); its tree's rows, and a table of the
    /// row store its rows, each record decoded straight into the vectors
    /// (`rows/gathered.rs`).
    fn scan_batches(self: Arc<Self>, tx: TxId, columns: &[usize], condition: Option<&Conjunction>) -> crate::storage::Batches {
        self.scan_batches_of(tx, columns, condition.cloned())
    }

    /// A table of the column store does.
    fn reads_by_batch(&self) -> bool {
        self.frozen.columns().is_some()
    }

    /// Of the column store, the columns COLUMNSTORE.ORDER names.
    fn ordered_by(&self) -> Vec<usize> {
        self.frozen.columns().map(|c| c.order.places().to_vec()).unwrap_or_default()
    }

    /// Of the column store, its groups' rows and the tree's merged in the
    /// order of `keys` (`rows/frozen/ordered.rs`).
    fn scan_ordered(self: Arc<Self>, tx: TxId, columns: &[usize], keys: &[usize], conditions: Option<&Conjunction>) -> Option<Rows> {
        (!keys.is_empty() && self.ordered_by().starts_with(keys)).then(|| self.scan_ordered_of(tx, (columns, keys), conditions.cloned()))
    }

    fn row_count(&self) -> usize {
        self.counted.live.load(Ordering::Acquire)
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

    /// 4.4 of a table the buffer pool holds, more as it holds less of it,
    /// shared with the other tables and indexes of its file, to 14.6; more
    /// by the share of the rows frozen, each a page a column
    /// (`rows/frozen.rs`).
    fn fetch_cost(&self) -> f64 {
        self.frozen_fetch_cost()
    }

    /// A table opened has none until the database's maintenance gathers
    /// them, due once first asked for (`rows/stats.rs`).
    fn statistics(&self) -> Option<Arc<dyn Statistics>> {
        let stats: Arc<dyn Statistics> = self.gathered.asked_for()?;
        Some(stats)
    }

    fn garbage(self: Arc<Self>, as_of: Timestamp) -> Rows {
        self.pending(Box::new(move |at, _| at <= as_of), None)
    }

    /// Writes the row's newest version outright (`rows/recovery.rs`).
    fn apply(&self, id: RowId, row: Option<Row>) -> Result<()> {
        self.apply_newest(id, row.clone())?;
        self.zones.widen(id, row.as_ref(), true);
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
    }
    fn reconfigure(&self, desc: &TableDesc) {
        self.frozen.reconfigure(options::columns_of(desc, self.frozen.columns()));
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
        self.note_commit(at);
    }

    fn rollback(&self, tx: TxId) -> Result<()> {
        self.end_open(tx)
    }

    fn read_by_condition(&self, tx: TxId, condition: &Condition) -> Result<()> {
        self.note_read(tx, Read::Condition(Conjunction::of(condition.clone())))
    }

    fn newest(&self, tx: TxId, id: RowId) -> Result<Newest> {
        self.newest_of(tx, id)
    }

    /// Statistics due are gathered too.
    fn uncollected(&self) -> bool {
        self.awaits_collection()
    }

    fn gc(&self, horizon: Timestamp, removed: &mut dyn FnMut(RowId, Row)) -> Result<()> {
        self.collect(horizon, removed)
    }

    /// Statistics to gather; zones to sum up; rows to freeze, groups to
    /// write again, entries to take out.
    fn maintenance_due(&self) -> bool {
        self.gather_wanted() || self.zones.due() || self.freeze_wanted()
    }

    /// Statistics gathered as one call's work, zones summed up as the
    /// next's, freezing as the next's.
    fn maintain(&self, horizon: Timestamp) -> Result<bool> {
        if self.gather_wanted() {
            self.gather_if_due();
            return Ok(true);
        }
        if self.zones.due() {
            return self.sum_zones(horizon);
        }
        self.freeze_due(horizon)
    }

    fn versions(self: Arc<Self>, horizon: Timestamp, as_of: Timestamp, columns: &[usize]) -> Rows {
        let unreadable = self.pending(Box::new(move |at, own| if own { at <= as_of } else { at <= horizon }), Some(columns));
        let frozen = match self.frozen.directory() {
            Ok(directory) => Merged::new(Arc::clone(&self), (Merging::FrozenOnly, directory), columns.to_vec(), None),
            Err(e) => return Box::new(std::iter::once(Err(e))),
        };
        let columns = columns.to_vec();
        // Held to the end, so no entry of a row frozen meanwhile is taken
        // out before it is read.
        let held = self.frozen.directory();
        Box::new(unreadable.chain(frozen).chain(Self::entries(&self).flat_map(move |entry| {
            let _held = &held;
            let found = entry.and_then(|(key, newest)| self.versions_of(row_id(&key)?, newest, (horizon, as_of), &columns));
            match found {
                Ok(found) => found.into_iter().map(Ok).collect::<Vec<_>>(),
                Err(e) => vec![Err(e)],
            }
        })))
    }
}

// Unit tests: a long value's page is checked against its row and column,
// a meta page against its format, an undone change keeps its older
// version for open readers, deleted rows are not written, versions and
// garbage are read as of a time, and a dropped table frees its pages
// (RULES.md R-TEST-1, R-TEST-2, R-TEST-7).
#[cfg(test)]
mod tests;
