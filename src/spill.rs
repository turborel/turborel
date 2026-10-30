//! \[DESIGN\] Where operators that outgrow memory put their rows (RULES.md
//! D38): an extension point, `Spill`. An operator that has used its work
//! area (the setting WORK_MEMORY) asks for an area of its own, reserved of
//! the database's memory budget, and makes in it what it needs:
//!
//! * a **table** (`SpillTable`): byte keys to byte values, a key's values
//!   in the order added, as a hash join's right operand or the rows of a
//!   set operation's second operand;
//! * a **run** (`SpillRun`): byte records read back in the order written,
//!   as a sort's sorted runs, or the rows a pass of grouping leaves to the
//!   next.
//!
//! Made of runs: a list that spills (`spill/list.rs`), and byte records
//! sorted (`spill/sort.rs`).
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

mod list;
mod paged;
mod sort;
#[cfg(test)]
pub(crate) mod testing;

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

pub use list::{Decode, Encode, ListItem, ListSpace, SpillList};
pub use paged::PagedSpill;
pub use sort::sorted_records;

use crate::bytes::Reader;
use crate::disk::encoding::Pipeline;
use crate::disk::io::FileSystem;
use crate::disk::memory::MemoryFileSystem;
use crate::error::Result;
use crate::memory::MemoryBudget;
use crate::storage::Row;
use crate::value::codec::{put_row, row};
use crate::value::types::TypeRegistry;
use crate::value::{Bucket, Decimal, NormKey, Value};

/// \[DESIGN\] The default WORK_MEMORY: what an operator holds in memory
/// before it spills, 4 MiB.
pub const DEFAULT_WORK_MEMORY: usize = 4 << 20;

/// \[DESIGN\] The default SPILL_BATCH: the bytes of records a run gathers
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

/// \[DESIGN\] A maker of spill areas: an extension point (RULES.md D38).
///
/// Contract (RULES.md R-SAFE-3):
/// * `area` reserves `area.bytes` of `memory` for the area while it lives,
///   refusing with SQLCODE -918 if the budget cannot grant them, and holds
///   no more memory than that, whatever its tables and runs hold: what does
///   not fit goes to storage, in files of `temp` if it keeps files.
/// * Everything an area made is gone once the area and its tables and runs
///   are dropped.
/// * Methods may be called from several threads at once; an area, a table
///   and a run are each used by one thread at a time.
/// * No method calls back into the `Database` (RULES.md R-ERR-4).
pub trait Spill: Send + Sync {
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

    /// Makes areas with `spill` from now on.
    pub(crate) fn set_spill(&self, spill: Arc<dyn Spill>) {
        self.place.write().unwrap_or_else(PoisonError::into_inner).0 = spill;
    }

    /// Keeps the files of areas made from now on in `temp`, once the files
    /// left there are removed (`TempFiles::clear`).
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if they cannot be.
    pub(crate) fn set_temp(&self, temp: TempFiles) -> Result<()> {
        temp.clear()?;
        let mut place = self.place.write().unwrap_or_else(PoisonError::into_inner);
        *temp.pipeline.write().unwrap_or_else(PoisonError::into_inner) = place.1.pipeline();
        place.1 = Arc::new(temp);
        Ok(())
    }

    /// Writes the files of areas made from now on through `pipeline`
    /// (`TempFiles::pipeline`).
    pub(crate) fn set_encodings(&self, pipeline: Pipeline) {
        let place = self.place.read().unwrap_or_else(PoisonError::into_inner);
        *place.1.pipeline.write().unwrap_or_else(PoisonError::into_inner) = pipeline;
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
    pipeline: RwLock<Pipeline>,
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
        TempFiles { fs, dir: dir.to_path_buf(), pipeline: RwLock::default(), prefix, next: AtomicU64::new(0) }
    }

    /// \[DESIGN\] The encodings its files are written through, as a spill
    /// provider makes them (RULES.md D48): an encryption with the
    /// process's ephemeral key while the database encrypts anything, so
    /// what spills of an encrypted table, or of encrypted files, never
    /// reaches the disk in the clear; none otherwise.
    pub fn pipeline(&self) -> Pipeline {
        self.pipeline.read().unwrap_or_else(PoisonError::into_inner).clone()
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
/// work area by.
pub fn row_bytes(row: &Row) -> usize {
    let values = row.len().saturating_mul(size_of::<Value>());
    let text: usize = row.iter().map(|v| if let Value::Char(s) = v { s.len() } else { 0 }).sum();
    values.saturating_add(text).saturating_add(size_of::<Row>())
}

/// `row` as a record.
pub fn row_record(row: &Row) -> Vec<u8> {
    let mut out = Vec::new();
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

/// The key of buckets `key`: the same bytes exactly when the buckets are.
pub fn bucket_key(key: &[Bucket]) -> Vec<u8> {
    let mut out = Vec::new();
    for b in key {
        match b {
            Bucket::Char(s) => text(&mut out, 0, s),
            Bucket::Number(bits) => {
                out.push(1);
                out.extend_from_slice(&bits.to_le_bytes());
            }
            Bucket::Instant(utc) => {
                out.push(4);
                out.extend_from_slice(&utc.to_le_bytes());
            }
            Bucket::Span(units) => {
                out.push(5);
                out.extend_from_slice(&units.to_le_bytes());
            }
            Bucket::Bits(held) => {
                out.push(3);
                out.extend_from_slice(&u64::try_from(held.len()).unwrap_or(u64::MAX).to_le_bytes());
                out.extend_from_slice(held);
            }
            Bucket::Extension(bytes) => {
                out.push(2);
                out.extend_from_slice(&u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_le_bytes());
                out.extend_from_slice(bytes);
            }
        }
    }
    out
}

/// The tags of a normalized key's values (`norm_key`).
const NULL_TAG: u8 = 0;
const CHAR_TAG: u8 = 1;
const EXACT_TAG: u8 = 2;
const APPROX_TAG: u8 = 3;
const EXTENSION_TAG: u8 = 4;
const BITS_TAG: u8 = 5;
const INSTANT_TAG: u8 = 6;
const SPAN_TAG: u8 = 7;

/// Appends `v`'s normalized key to `out`: the bytes `norm_key` gives of
/// `v.norm()`, made without making it.
pub fn put_norm(v: &Value, out: &mut Vec<u8>) {
    match v {
        Value::Null => out.push(NULL_TAG),
        Value::Char(s) => put_norm_text(s, out),
        Value::Exact(d) => put_norm_exact(d.units, d.scale, out),
        Value::Approx(f) => put_norm_approx(*f, out),
        Value::Bits(b) => {
            out.push(BITS_TAG);
            b.put_key(out);
        }
        Value::Datetime(d) => {
            out.push(INSTANT_TAG);
            out.extend_from_slice(&d.utc.to_le_bytes());
        }
        Value::Interval(i) => {
            out.push(SPAN_TAG);
            out.extend_from_slice(&i.units.to_le_bytes());
        }
        Value::Extension(e) => {
            text(out, EXTENSION_TAG, e.kind.name());
            out.extend_from_slice(&u64::try_from(e.bytes.len()).unwrap_or(u64::MAX).to_le_bytes());
            out.extend_from_slice(&e.bytes);
        }
    }
}

/// Appends the normalized key of the character string `s`, as `put_norm`
/// writes it of the value: without its trailing spaces, as strings
/// compare padded [X3.135-1986 §5.11 GR5].
pub fn put_norm_text(s: &str, out: &mut Vec<u8>) {
    text(out, CHAR_TAG, s.trim_end_matches(' '));
}

/// Appends the normalized key of the approximate number `f`, as
/// `put_norm` writes it of the value: -0 as 0, as they compare equal.
pub fn put_norm_approx(f: f64, out: &mut Vec<u8>) {
    out.push(APPROX_TAG);
    out.extend_from_slice(&(if f == 0.0 { 0f64.to_bits() } else { f.to_bits() }).to_le_bytes());
}

/// Appends the normalized key of the exact number of `units` at `scale`,
/// as `put_norm` writes it of the value, so an exact number held as an
/// integer keys as its value does.
pub fn put_norm_exact(units: i128, scale: u32, out: &mut Vec<u8>) {
    let (units, scale) = Decimal::new(units, scale).normalized();
    out.push(EXACT_TAG);
    out.extend_from_slice(&units.to_le_bytes());
    out.extend_from_slice(&scale.to_le_bytes());
}

/// The key of `values`: the bytes `norm_key` gives of their normalized
/// values, made without making them.
pub fn norm_row(values: &[Value]) -> Vec<u8> {
    // Most values' keys are a tag and 16 bytes or fewer.
    let mut out = Vec::with_capacity(values.len().saturating_mul(NORM_BYTES));
    for v in values {
        put_norm(v, &mut out);
    }
    out
}

/// The bytes `norm_row` makes room for of each value.
const NORM_BYTES: usize = 24;

/// The key of normalized values `key`: the same bytes exactly when the
/// values are identical (`Value::norm`).
pub fn norm_key(key: &[NormKey]) -> Vec<u8> {
    let mut out = Vec::new();
    for k in key {
        match k {
            NormKey::Null => out.push(NULL_TAG),
            NormKey::Char(s) => text(&mut out, CHAR_TAG, s),
            NormKey::Exact(units, scale) => {
                out.push(EXACT_TAG);
                out.extend_from_slice(&units.to_le_bytes());
                out.extend_from_slice(&scale.to_le_bytes());
            }
            NormKey::Approx(bits) => {
                out.push(APPROX_TAG);
                out.extend_from_slice(&bits.to_le_bytes());
            }
            NormKey::Bits(b) => {
                out.push(BITS_TAG);
                b.put_key(&mut out);
            }
            NormKey::Instant(utc) => {
                out.push(INSTANT_TAG);
                out.extend_from_slice(&utc.to_le_bytes());
            }
            NormKey::Span(units) => {
                out.push(SPAN_TAG);
                out.extend_from_slice(&units.to_le_bytes());
            }
            NormKey::Extension(held) => {
                let (name, bytes) = &**held;
                text(&mut out, EXTENSION_TAG, name);
                out.extend_from_slice(&u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_le_bytes());
                out.extend_from_slice(bytes);
            }
        }
    }
    out
}

/// Appends `tag`, `s`'s length and `s`.
fn text(out: &mut Vec<u8>, tag: u8, s: &str) {
    out.push(tag);
    out.extend_from_slice(&u64::try_from(s.len()).unwrap_or(u64::MAX).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}

// Unit tests: a value's normalized key, made in place, is the one made of
// its normalized value (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
