//! \[DESIGN\] Where operators past their work area (MEMORY.WORK_BYTES) put
//! their rows (RULES.md D38), an extension point (`Spill`): each makes, in
//! an area of its own reserved of the database's memory budget, what it needs:
//!
//! * a **table** (`SpillTable`): byte keys to byte values, a key's values
//!   in the order added, as a hash join's right operand or the rows of a
//!   set operation's second operand;
//! * a **run** (`SpillRun`): byte records read back in the order written,
//!   as a sort's sorted runs, or the rows a pass of grouping leaves to the
//!   next.
//!
//! Made of runs: a list that spills (`spill/list.rs`), and byte records
//! sorted (`spill/sort.rs`); of a table, a map that spills (`spill/map.rs`).
//!
//! The built-in provider (`spill/paged.rs`) keeps an area in a temporary
//! pager file of its own, through a buffer pool of the area's size, so
//! what does not fit the pool is written to the file as pages are evicted;
//! tables are linear hash tables (`engine/hash.rs`). A plug-in may give
//! others, partitioned or parallel (`Database::set_spill`).
//!
//! Rows and keys are written in self-describing byte forms
//! (`value/codec.rs`, and here): keys so that two keys are the same bytes
//! exactly when the operator would find them equal.

mod keys;
mod list;
pub(crate) mod map;
pub(crate) mod paged;
mod sort;
#[cfg(test)]
pub(crate) mod testing;

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

pub use keys::{bucket_key, norm_key, norm_row, put_norm, put_norm_approx, put_norm_exact, put_norm_text};
pub use list::{Decode, Encode, ListItem, ListSpace, Located, SpillList};
pub use paged::PagedSpill;
pub use sort::sorted_records;

use crate::bytes::Reader;
use crate::disk::encoding::{LivePipeline, Pipeline};
use crate::disk::io::FileSystem;
use crate::disk::memory::MemoryFileSystem;
use crate::error::Result;
use crate::memory::MemoryBudget;
use crate::storage::Row;
use crate::value::Value;
use crate::value::codec::{put_row, row};
use crate::value::types::TypeRegistry;

/// \[DESIGN\] The default MEMORY.WORK_BYTES: what an operator holds in memory
/// before it spills, 4 MiB.
pub const DEFAULT_WORK_MEMORY: usize = 4 << 20;

/// \[DESIGN\] The default SPILL.BATCH_BYTES: the bytes of records a run gathers
/// before it writes them, 16 KiB.
pub const DEFAULT_SPILL_BATCH: usize = 16 << 10;

/// \[DESIGN\] What a spill area is asked to be. More may be added, so it
/// is made with `Area::new` and read by its fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Area {
    /// The bytes it holds in memory, reserved of the budget.
    pub bytes: usize,
    /// The bytes of records a run gathers before it writes them.
    pub batch: usize,
}

impl Area {
    /// An area of `bytes`, its runs written in batches of `batch` bytes.
    pub fn new(bytes: usize, batch: usize) -> Area {
        Area { bytes, batch }
    }
}

/// Records read back from a run.
pub type Records = Box<dyn Iterator<Item = Result<Vec<u8>>> + Send>;

/// Records read back from a run a batch at a time.
pub type Batches = Box<dyn Iterator<Item = Result<Batch>> + Send>;

/// Records of a run read back together (`SpillRun::into_batches`): their
/// bytes in one buffer, and where each lies in it, so a reader takes each
/// where it lies rather than as a copy of its own.
#[derive(Debug, Default)]
pub struct Batch {
    bytes: Vec<u8>,
    bounds: Vec<(usize, usize)>,
}

impl Batch {
    /// The records of `bytes` that `bounds` gives, each its start and end.
    pub fn new(bytes: Vec<u8>, bounds: Vec<(usize, usize)>) -> Batch {
        Batch { bytes, bounds }
    }

    /// A batch of one record.
    pub fn one(record: Vec<u8>) -> Batch {
        let end = record.len();
        Batch { bytes: record, bounds: vec![(0, end)] }
    }

    /// How many records it holds.
    pub fn len(&self) -> usize {
        self.bounds.len()
    }

    /// Whether it holds none.
    pub fn is_empty(&self) -> bool {
        self.bounds.is_empty()
    }

    /// Its record `at`, if it has one there that lies within its bytes.
    pub fn get(&self, at: usize) -> Option<&[u8]> {
        let &(start, end) = self.bounds.get(at)?;
        self.bytes.get(start..end)
    }
}

/// \[DESIGN\] A maker of spill areas: an extension point (RULES.md D38).
///
/// Contract (RULES.md R-SAFE-3):
/// * `area` reserves `area.bytes` of `memory` for the area while it lives,
///   refusing with SQLCODE -918 if the budget cannot grant them, and holds
///   no more memory than that, whatever its tables and runs hold: what does
///   not fit goes to storage, in files of `temp` if it keeps files,
///   written through the encodings `TempFiles::pipeline` gives as each
///   part is written, of an area made before a change too (RULES.md D48).
/// * Everything an area made is gone once the area and its tables and runs
///   are dropped.
/// * Methods may be called from several threads at once; an area, a table
///   and a run are each used by one thread at a time.
/// * No method calls back into the `Database` (RULES.md R-ERR-4).
pub trait Spill: Send + Sync {
    /// Its name, for messages and its settings.
    fn name(&self) -> &'static str;
    /// \[DESIGN\] Itself as it declares settings and is told them
    /// (`Configurable`, RULES.md D36), if it has any. Default: none.
    fn configurable(&self) -> Option<&dyn crate::settings::Configurable> {
        None
    }
    /// A new area as `area` asks, reserved of `memory`, its files in
    /// `temp`.
    ///
    /// # Errors
    ///
    /// SQLCODE -918 if the budget cannot grant it; its storage's errors.
    fn area(&self, memory: &Arc<MemoryBudget>, temp: &Arc<TempFiles>, area: Area) -> Result<Box<dyn SpillArea>>;
}

/// An area an operator spills to.
pub trait SpillArea: Send {
    /// A new, empty table.
    ///
    /// # Errors
    ///
    /// Its storage's errors.
    fn table(&self) -> Result<Box<dyn SpillTable>>;
    /// A new, empty run.
    ///
    /// # Errors
    ///
    /// Its storage's errors.
    fn run(&self) -> Result<Box<dyn SpillRun>>;
}

/// A table of a spill area: byte keys to byte values, a key's values in
/// the order added, of any length.
pub trait SpillTable: Send {
    /// Adds `value` under `key`, after any there.
    ///
    /// # Errors
    ///
    /// Its storage's errors.
    fn insert(&mut self, key: &[u8], value: &[u8]) -> Result<()>;
    /// The values under `key`, in the order added.
    ///
    /// # Errors
    ///
    /// Its storage's errors.
    fn get(&self, key: &[u8]) -> Result<Vec<Vec<u8>>>;
    /// The values under `key`, in the order added, read as they are taken,
    /// so a key of more values than memory holds is read whole: as a hash
    /// join probes a key that most of its right rows have. Nothing is added
    /// or removed while they are read. Default: those `get` gives.
    ///
    /// # Errors
    ///
    /// Its storage's errors.
    fn values(&self, key: &[u8]) -> Result<Records> {
        Ok(Box::new(self.get(key)?.into_iter().map(Ok)))
    }
    /// Removes the first `value` under `key`; whether there was one.
    ///
    /// # Errors
    ///
    /// Its storage's errors.
    fn remove(&mut self, key: &[u8], value: &[u8]) -> Result<bool>;
}

/// A run of a spill area: records read back in the order written.
pub trait SpillRun: Send {
    /// Adds `record` at the end.
    ///
    /// # Errors
    ///
    /// Its storage's errors.
    fn push(&mut self, record: &[u8]) -> Result<()>;
    /// Its records, in order, from the first: no more are pushed after.
    /// It may be read so again.
    ///
    /// # Errors
    ///
    /// Its storage's errors.
    fn records(&mut self) -> Result<Records>;
    /// Its records, in order, read once: what holds each may be freed as it
    /// is read, so a merge of many passes holds no more than it has still
    /// to read. Default: `records`.
    ///
    /// # Errors
    ///
    /// Its storage's errors.
    fn into_records(mut self: Box<Self>) -> Result<Records> {
        self.records()
    }
    /// Its records, in order, read once as `into_records` reads them, but a
    /// batch at a time, each record read where it lies in its batch: as a
    /// sort's merge reads them, sparing a copy of each. Default: each record
    /// of `into_records` a batch of its own.
    ///
    /// # Errors
    ///
    /// Its storage's errors.
    fn into_batches(self: Box<Self>) -> Result<Batches> {
        Ok(Box::new(self.into_records()?.map(|r| r.map(Batch::one))))
    }
}

/// \[DESIGN\] Where a database's operators, and its plug-ins (`Host`), get
/// their spill areas: its spill provider, the directory of their files,
/// and its memory budget. Shared by every snapshot of the database, and
/// changed in place when a plug-in replaces the provider or the database is
/// opened from its directory.
pub struct WorkAreas {
    memory: Arc<MemoryBudget>,
    place: RwLock<(Arc<dyn Spill>, Arc<TempFiles>)>,
}

impl WorkAreas {
    /// The built-in provider's areas, their files in memory, reserved of
    /// `memory` as are their pools (RULES.md D56).
    pub(crate) fn new(memory: Arc<MemoryBudget>) -> WorkAreas {
        let files = MemoryFileSystem::within(Arc::clone(&memory), "a spill file");
        let temp = TempFiles::new(Arc::new(files), Path::new("/tmp"));
        WorkAreas { memory, place: RwLock::new((Arc::new(PagedSpill), Arc::new(temp))) }
    }

    /// A new area as `area` asks, reserved of the budget.
    ///
    /// # Errors
    ///
    /// SQLCODE -918 if the budget cannot grant it; the provider's errors.
    pub fn area(&self, area: Area) -> Result<Box<dyn SpillArea>> {
        let (spill, temp) = self.place.read().unwrap_or_else(PoisonError::into_inner).clone();
        spill.area(&self.memory, &temp, area)
    }

    /// What makes its areas.
    pub(crate) fn spill(&self) -> Arc<dyn Spill> {
        Arc::clone(&self.place.read().unwrap_or_else(PoisonError::into_inner).0)
    }

    /// Makes areas with `spill` from now on.
    pub(crate) fn set_spill(&self, spill: Arc<dyn Spill>) {
        self.place.write().unwrap_or_else(PoisonError::into_inner).0 = spill;
    }

    /// Keeps the files of areas made from now on in `temp`, once the files
    /// left there are removed (`TempFiles::clear`): their encodings the
    /// ones before, shared, so a change reaches files made before too.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if they cannot be.
    pub(crate) fn set_temp(&self, mut temp: TempFiles) -> Result<()> {
        temp.clear()?;
        let mut place = self.place.write().unwrap_or_else(PoisonError::into_inner);
        temp.pipeline = Arc::clone(&place.1.pipeline);
        place.1 = Arc::new(temp);
        Ok(())
    }

    /// Where scratch files are made: a plug-in's own, removed when it is
    /// done with them, and cleared with the spill areas' as the database
    /// opens and closes if it is not (RULES.md D56).
    pub fn scratch(&self) -> Arc<TempFiles> {
        Arc::clone(&self.place.read().unwrap_or_else(PoisonError::into_inner).1)
    }

    /// Removes the files of spill areas left in its directory, as the
    /// database closes (`TempFiles::clear`).
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if one cannot be removed.
    pub(crate) fn clear_temp(&self) -> Result<()> {
        let temp = Arc::clone(&self.place.read().unwrap_or_else(PoisonError::into_inner).1);
        temp.clear()
    }

    /// Writes the pages of areas' files through `pipeline` from now on
    /// (`TempFiles::pipeline`): of areas made before too (RULES.md D48).
    pub(crate) fn set_encodings(&self, pipeline: Pipeline) {
        let temp = Arc::clone(&self.place.read().unwrap_or_else(PoisonError::into_inner).1);
        temp.pipeline.set(pipeline);
    }
}

impl fmt::Debug for WorkAreas {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let dir = self.place.read().unwrap_or_else(PoisonError::into_inner).1.dir().to_path_buf();
        f.debug_struct("WorkAreas").field("dir", &dir).finish_non_exhaustive()
    }
}

/// \[DESIGN\] Where a database's spill areas keep their files: a directory
/// of a file system, each file named apart from every other, in this
/// process and others. A database kept in a directory has `tmp/` in it; one
/// in memory has a file system in memory, never the system's temporary
/// directory.
pub struct TempFiles {
    fs: Arc<dyn FileSystem>,
    dir: PathBuf,
    /// The encodings its files pass through (`WorkAreas::set_encodings`).
    pipeline: Arc<LivePipeline>,
    /// Tells these files apart from other databases' of this process.
    prefix: String,
    next: AtomicU64,
}

/// Numbers the `TempFiles` of this process.
static MADE: AtomicU64 = AtomicU64::new(0);

impl TempFiles {
    /// Files in `dir` of `fs`.
    pub fn new(fs: Arc<dyn FileSystem>, dir: &Path) -> TempFiles {
        let prefix = format!("turborel-{}-{}", std::process::id(), MADE.fetch_add(1, Ordering::Relaxed));
        TempFiles { fs, dir: dir.to_path_buf(), pipeline: Arc::default(), prefix, next: AtomicU64::new(0) }
    }

    /// \[DESIGN\] The encodings its files are written through, as a spill
    /// provider makes them (RULES.md D48): those SPILL.ENCODINGS names, as
    /// a compression; then an encryption with the process's ephemeral key
    /// while the database encrypts anything, so what spills of an
    /// encrypted table, or of encrypted files, never reaches the disk in
    /// the clear.
    ///
    /// It changes while files are written: a provider writes each page, or
    /// block, of a file it made before through the one it holds then, as
    /// the built-in one does (`Pager::create_scratch`), so nothing reaches
    /// the disk in the clear once anything is encrypted.
    pub fn pipeline(&self) -> Pipeline {
        self.pipeline.get()
    }

    /// The encodings its files pass through, as they change.
    pub fn live_pipeline(&self) -> &Arc<LivePipeline> {
        &self.pipeline
    }

    /// Its file system.
    pub fn fs(&self) -> &Arc<dyn FileSystem> {
        &self.fs
    }

    /// Its directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Removes the files of spill areas left in its directory by a process
    /// that stopped before it could: the files of a database's own
    /// directory, which one process opens at a time.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if the directory cannot be read or a file removed.
    pub fn clear(&self) -> Result<()> {
        if !self.fs.exists(&self.dir)? {
            return Ok(());
        }
        for path in self.fs.read_dir(&self.dir)? {
            if path.extension().is_some_and(|e| e == "spill") {
                self.fs.remove(&path)?;
            }
        }
        Ok(())
    }

    /// The path of a new file, used by no other.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if the directory cannot be made.
    pub fn path(&self) -> Result<PathBuf> {
        self.fs.create_dir_all(&self.dir)?;
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        Ok(self.dir.join(format!("{}-{n}.spill", self.prefix)))
    }
}

/// The bytes `row` holds in memory, near enough to bound an operator's
/// work area by: its values, and what each holds apart from itself.
pub fn row_bytes(row: &Row) -> usize {
    let values = row.len().saturating_mul(size_of::<Value>());
    let held = row.iter().fold(0usize, |sum, v| sum.saturating_add(heap_bytes(v)));
    values.saturating_add(held).saturating_add(size_of::<Row>())
}

/// The bytes `v` holds apart from itself: a string's characters, a bit
/// string's bytes, a plug-in's value's bytes (counted by every value that
/// shares them, so never too few). The others hold all they have in
/// themselves.
pub fn heap_bytes(v: &Value) -> usize {
    match v {
        Value::Char(s) => s.len(),
        Value::Bits(b) => b.bytes().len(),
        Value::Extension(e) => e.bytes.len(),
        Value::Null | Value::Exact(_) | Value::Approx(_) | Value::Datetime(_) | Value::Interval(_) => 0,
    }
}

/// The bytes a record holds for each value of a row, room made for them
/// at once: most values' records are a tag and 20 bytes or fewer
/// (`codec::put_value`), so a record is seldom grown as it is written.
const VALUE_RECORD_BYTES: usize = 24;

/// The bytes of a row's record before its values: their count, a `u32`.
const ROW_HEAD_BYTES: usize = 4;

/// The room to make for a record of `more` bytes, then a row of `values`
/// values (`row_record`), so it is seldom grown as it is written.
pub fn record_room(more: usize, values: usize) -> usize {
    values.saturating_mul(VALUE_RECORD_BYTES).saturating_add(ROW_HEAD_BYTES).saturating_add(more)
}

/// `row` as a record.
pub fn row_record(row: &Row) -> Vec<u8> {
    let mut out = Vec::with_capacity(record_room(0, row.len()));
    put_row(&mut out, row);
    out
}

/// The row of `record`, its values of types beyond the standard found
/// among `types`.
///
/// # Errors
///
/// SQLCODE -916 if it is not a row's record.
pub fn record_row(record: &[u8], types: &TypeRegistry) -> Result<Row> {
    row(&mut Reader::new(record, "spilled row").with_types(types))
}

// Unit tests: a value's normalized key, made in place, is the one made of
// its normalized value (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
