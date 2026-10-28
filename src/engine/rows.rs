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
//! * **Long values** go out of line (`record.rs`): a character value
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
//!   values out of line.
//!
//! Durability is the log's (D25): the table is `Logged`, durable through
//! the store's last checkpoint (`StorageEngine::checkpoint`), taken when
//! the database is closed cleanly; its locator, the meta page and that
//! position, finds it again in its file (`StorageEngine::open_table`).

mod discard;
mod files;
mod frozen;
mod ids;
mod meta;
mod mvcc;
mod options;
mod recovery;
mod scan;
mod sealing;
mod stats;
mod store;
mod undo;
mod values;
mod version;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};

pub use files::TableFiles;
use frozen::{Frozen, Merged, Merging};
use ids::{BLOCK, IdBlocks};
use ids::{key, no_row, row_id};
pub use meta::TABLE_FORMAT;
use meta::{locator_of, meta_content, parse_locator, parse_meta};
use mvcc::{Read, Versions};
pub use options::{DEFAULT_HELD_BYTES, DEFAULT_TRACKED_ROWS, Refresh, TableOptions};
use sealing::Sealing;
use stats::Gathered;
pub use undo::UndoStore;

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
use crate::spill::WorkAreas;
use crate::stats::Statistics;
use crate::storage::{Condition, Newest, Row, RowId, Rows, StorageEngine, TableStorage};
use crate::sync::{lock, lock_builtin, write};
use crate::txn::{Concurrency, Persistence, ReadMode, Timestamp, TxCapabilities, TxId, Undo};
use crate::value::DataType;

/// The name of the row store.
pub const ROW_STORE: &str = "rowstore";
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
}

impl StorageEngine for RowStore {
    fn name(&self) -> &'static str {
        ROW_STORE
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

    /// Writes back every page and the pager's free list; a file shadowed
    /// (`TableFiles::shadowed`) holds them in memory and seals that batch,
    /// its commit left to `checkpoint_finish`; one not shadowed is synced
    /// now.
    fn checkpoint(&self, at: Lsn) -> Result<()> {
        let files = self.files()?;
        // No table's rows half frozen (`rows/frozen.rs`).
        let _frozen = write(&files.freezes)?;
        if let Some(shadows) = &files.shadows {
            // What is written now is kept in memory: no I/O with writers held.
            shadows.hold()?;
            if let Err(e) = files.pool.sync(files.file, Durability::Full).and_then(|()| shadows.seal()) {
                let _ = shadows.release();
                return Err(e);
            }
            files.sealed.store(at.0, Ordering::Release);
            return Ok(());
        }
        files.pool.sync(files.file, Durability::Full)?;
        files.durable.fetch_max(at.0, Ordering::AcqRel);
        Ok(())
    }

    /// Commits the batch sealed, so after a crash the file holds exactly
    /// what the checkpoint captured.
    fn checkpoint_finish(&self) -> Result<()> {
        let files = self.files()?;
        if let Some(shadows) = &files.shadows {
            shadows.commit_sealed()?;
            files.durable.fetch_max(files.sealed.load(Ordering::Acquire), Ordering::AcqRel);
        }
        Ok(())
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
    live: AtomicUsize,
    /// Its statistics (`rows/stats.rs`).
    gathered: Gathered,
    /// The store's durable position (`TableFiles`).
    durable: Arc<AtomicU64>,
    /// Set when the table is dropped: its pages are freed when it goes
    /// (`rows/discard.rs`).
    discarded: AtomicBool,
    /// How its rows are sealed (`rows/sealing.rs`).
    sealing: Sealing,
    /// Its frozen row groups, of the COLUMNS layout (`rows/frozen.rs`).
    frozen: Frozen,
    /// Held shared while it freezes rows (`TableFiles::freezes`).
    freezes: Arc<RwLock<()>>,
}

/// What a table's meta page names, made or read.
struct Found {
    meta: PageId,
    tree: BLinkTree,
    /// The row ids reserved.
    reserved: u64,
}

impl RowTable {
    /// Creates an empty table of columns of `types` in `files`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the file is not a registered pager file; the buffer
    /// pool's errors.
    pub fn create(files: &TableFiles, types: &[DataType], options: TableOptions) -> Result<RowTable> {
        let tree = BLinkTree::create(Arc::clone(&files.pool), files.file)?;
        let meta = {
            let (meta, mut page) = files.pool.new_page(files.file)?;
            *page = meta_content(tree.meta(), RESERVE, 0);
            meta
        };
        RowTable::with(files, Found { meta, tree, reserved: RESERVE }, types, options)
    }

    /// Opens the table whose meta page is `meta`, of columns of `types`:
    /// row ids go on past those it reserved, and its rows are counted in
    /// the pass that gathers its statistics.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if `meta` is not a table's meta page, or a row cannot
    /// be read; as `create`.
    pub fn open(files: &TableFiles, meta: PageId, types: &[DataType], options: TableOptions) -> Result<RowTable> {
        RowTable::open_with(files, (meta, types), options, Sealing::plain())
    }

    /// As `open`, its rows sealed as `sealing` says, its frozen row groups'
    /// pages too, which are read as it opens.
    fn open_with(files: &TableFiles, (meta, types): (PageId, &[DataType]), options: TableOptions, sealing: Sealing) -> Result<RowTable> {
        let (tree_meta, reserved, directory) = {
            let page = files.pool.read(files.file, meta.0)?;
            parse_meta(&page).ok_or_else(|| corrupt(&format!("page {} is not a table's meta page of this format", meta.0)))?
        };
        let tree = BLinkTree::open(Arc::clone(&files.pool), files.file, tree_meta)?;
        let table = RowTable::with(files, Found { meta, tree, reserved }, types, options)?.sealed(sealing);
        table.load_directory(directory)?;
        table.live.store(table.count_newest()?, Ordering::Release);
        table.gathered.gather_when_asked();
        Ok(table)
    }

    fn with(files: &TableFiles, found: Found, types: &[DataType], options: TableOptions) -> Result<RowTable> {
        let Found { meta, tree, reserved } = found;
        let threshold = options.out_of_line.unwrap_or(files.pool.capacity(files.file)?.checked_div(4).unwrap_or_default());
        // A row's record shares its leaf entry with its key and its version.
        let spill = Spill { threshold, most: tree.max_entry().saturating_sub(KEY).saturating_sub(VERSION) };
        Ok(RowTable {
            pool: Arc::clone(&files.pool),
            file: files.file,
            undo: Arc::clone(&files.undo),
            versions: Versions::bounded(options.bounds(), Arc::clone(&files.work)),
            meta,
            tree,
            format: RecordFormat::new(types),
            spill,
            next: AtomicU64::new(reserved),
            blocks: IdBlocks::new(),
            reserved: AtomicU64::new(reserved),
            reserving: Mutex::new(()),
            live: AtomicUsize::new(0),
            gathered: Gathered::new(options.statistics),
            durable: Arc::clone(&files.durable),
            discarded: AtomicBool::new(false),
            sealing: Sealing::plain(),
            frozen: Frozen::new(options.columns),
            freezes: Arc::clone(&files.freezes),
        })
    }

    /// The table, its rows sealed as `sealing` says.
    fn sealed(mut self, sealing: Sealing) -> RowTable {
        self.sealing = sealing;
        self
    }

    /// Its meta page, by which `open` finds it.
    pub fn meta(&self) -> PageId {
        self.meta
    }
}

impl RowTable {
    /// A new row id.
    fn next_id(&self) -> Result<RowId> {
        self.blocks.next(|| {
            let start = self.next.fetch_add(BLOCK, Ordering::AcqRel);
            let last = start.checked_add(BLOCK - 1).filter(|&last| last < RowId::MAX);
            let last = last.ok_or_else(|| SqlError::new(Code::Unsupported, "[DESIGN] the table has given every row id"))?;
            self.reserve_past(last)?;
            Ok(start)
        })
    }

    /// Makes the meta page record `id` as possibly given.
    fn reserve_past(&self, id: RowId) -> Result<()> {
        if id < self.reserved.load(Ordering::Acquire) {
            return Ok(());
        }
        let _reserving = lock(&self.reserving)?;
        if id < self.reserved.load(Ordering::Acquire) {
            return Ok(());
        }
        let reserved = id.saturating_add(RESERVE);
        *self.pool.write(self.file, self.meta)? = meta_content(self.tree.meta(), reserved, self.frozen.stored());
        self.reserved.store(reserved, Ordering::Release);
        Ok(())
    }

    /// Writes the meta page as the table is now.
    fn write_meta(&self) -> Result<()> {
        let _reserving = lock(&self.reserving)?;
        *self.pool.write(self.file, self.meta)? =
            meta_content(self.tree.meta(), self.reserved.load(Ordering::Acquire), self.frozen.stored());
        Ok(())
    }
}

impl TableStorage for RowTable {
    fn insert(&self, tx: TxId, row: Row) -> Result<RowId> {
        let id = self.next_id()?;
        let (record, pages) = self.encode(id, &row)?;
        match self.insert_version(tx, id, &record) {
            Ok(true) => {
                self.live.fetch_add(1, Ordering::AcqRel);
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
        self.gathered.changed();
        Ok(())
    }

    fn delete(&self, tx: TxId, id: RowId) -> Result<()> {
        self.write_version(tx, id, None)?;
        self.live.fetch_sub(1, Ordering::AcqRel);
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
                self.live.fetch_add(1, Ordering::AcqRel);
                Ok(())
            }
            Ok(false) => self.write_version(tx, id, Some(&record)),
            Err(e) => Err(e),
        };
        restored.inspect_err(|_| self.free(&pages))
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
    fn scan_where(self: Arc<Self>, tx: TxId, columns: &[usize], condition: &Condition) -> Rows {
        self.scan_rows(tx, columns, Some(condition.clone()))
    }

    fn row_count(&self) -> usize {
        self.live.load(Ordering::Acquire)
    }

    /// A table opened gathers them when first asked (`rows/recovery.rs`).
    fn statistics(&self) -> Option<Arc<dyn Statistics>> {
        let stats: Arc<dyn Statistics> = match self.gathered.get() {
            Some(stats) => stats,
            None if self.gathered.asked() => self.gather().ok()?,
            None => return None,
        };
        Some(stats)
    }

    fn garbage(self: Arc<Self>, as_of: Timestamp) -> Rows {
        self.pending(Box::new(move |at, _| at <= as_of), None)
    }

    /// Writes the row's newest version outright (`rows/recovery.rs`).
    fn apply(&self, id: RowId, row: Option<Row>) -> Result<()> {
        self.apply_newest(id, row)
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
        self.note_read(tx, Read::Condition(condition.clone()))
    }

    fn newest(&self, tx: TxId, id: RowId) -> Result<Newest> {
        self.newest_of(tx, id)
    }

    /// Statistics due are gathered too.
    fn uncollected(&self) -> bool {
        self.awaits_collection()
    }

    /// Freezes rows due, too: one that cannot be is left for the next.
    fn gc(&self, horizon: Timestamp, removed: &mut dyn FnMut(RowId, Row)) -> Result<()> {
        self.collect(horizon, removed)?;
        let _ = self.freeze_due(horizon);
        self.gather_if_due();
        Ok(())
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
// a meta page against its format, and an undone change keeps its older
// version for open readers (RULES.md R-TEST-1, R-TEST-2, R-TEST-7).
#[cfg(test)]
mod tests;
