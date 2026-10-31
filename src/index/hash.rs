//! \[DESIGN\] The built-in hash index method, "hash": each index a linear
//! hash table in pages (`engine/hash.rs`), for equality of its whole key.
//! The indexes of a database kept in a directory are in its data file,
//! beside its tables and B-link trees, so they are reopened and
//! checkpointed with them; those of a database kept nowhere else are in a
//! file in memory.
//!
//! An entry is the encoded key (`engine/key.rs`), under which values equal
//! by the [X3.135-1986 §5.11] comparison rules are the same bytes, and its
//! row id as value; a (key, row) pair inserted twice is two entries, and a
//! remove takes back one. A probe value of its column's type is looked up
//! by its key; one not exactly of it (a number of a finer scale, a string
//! longer than the column) by its anchor, each entry found checked; one of
//! another kind is checked against every entry, a bucket at a time as the
//! entries are taken (`engine/hash/scan.rs`).

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use super::stats::KeyCounts;
use super::{Entries, HASH_METHOD, Index, IndexMethod, Key, KeyOrder, Probe};
use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::io::Durability;
use crate::disk::pager::PageId;
use crate::engine::hash::HashTable;
use crate::engine::key::{anchor, decode_key, encode_key};
use crate::engine::{MEMORY_POOL_BYTES, MemoryPages};
use crate::error::{Code, Result, SqlError};
use crate::memory::{Host, MemoryBudget};
use crate::storage::RowId;
use crate::sync::lock_builtin;
use crate::value::{CompOp, DataType, Value};

/// Bytes of a row id, an entry's value.
const ROW_ID: usize = 8;

/// \[DESIGN\] The built-in hash index method: hash tables in one pager
/// file of a buffer pool.
pub struct HashMethod {
    /// Where its indexes are; for a method in memory, made when the first
    /// index is.
    files: Mutex<Option<(Arc<BufferPool>, FileId)>>,
    /// Its pages in memory, for a file made in memory: of a budget of its
    /// own until it is attached to a database's (`attach`).
    memory: Arc<MemoryPages>,
    /// Its indexes not yet dropped, whose counts a close writes to their
    /// meta pages.
    tables: Mutex<Vec<Weak<HashTable>>>,
}

impl HashMethod {
    /// A method keeping its indexes in pager file `file` of `pool`.
    pub fn new(pool: Arc<BufferPool>, file: FileId) -> HashMethod {
        HashMethod { files: Mutex::new(Some((pool, file))), memory: MemoryPages::new(0), tables: Mutex::new(Vec::new()) }
    }

    /// \[DESIGN\] A method keeping its indexes in memory, in a file system of
    /// its own: the built-in method of a database kept nowhere else. Its
    /// file is made when its first index is, so making it cannot fail; its
    /// pool (`MEMORY_POOL_BYTES`) reserved then, of the budget of the
    /// database it is attached to, or of its own.
    pub fn in_memory() -> HashMethod {
        HashMethod::sharing(MemoryPages::new(MEMORY_POOL_BYTES))
    }

    /// A method keeping its indexes in `memory`, which others may share.
    pub(crate) fn sharing(memory: Arc<MemoryPages>) -> HashMethod {
        HashMethod { files: Mutex::new(None), memory, tables: Mutex::new(Vec::new()) }
    }

    /// Makes `budget` the one its pool in memory draws on, if made after.
    pub(crate) fn use_budget(&self, budget: Arc<MemoryBudget>) {
        self.memory.use_budget(budget);
    }

    /// Its file, made in memory if it has none yet.
    fn files(&self) -> Result<(Arc<BufferPool>, FileId)> {
        let mut files = lock_builtin(&self.files);
        if let Some((pool, file)) = files.as_ref() {
            return Ok((Arc::clone(pool), *file));
        }
        let (pool, file) = self.memory.file("the pool in memory of the built-in engine and index methods")?;
        *files = Some((Arc::clone(&pool), file));
        Ok((pool, file))
    }

    /// `table` as an index on columns of `types`, its keys counted by
    /// `keys`, kept for checkpoints.
    fn index(&self, (table, keys): (HashTable, KeyCounts), types: &[DataType]) -> Box<dyn Index> {
        let table = Arc::new(table);
        let mut tables = lock_builtin(&self.tables);
        tables.retain(|t| t.strong_count() > 0);
        tables.push(Arc::downgrade(&table));
        Box::new(HashIndex { table, types: types.to_vec(), discarded: AtomicBool::new(false), keys })
    }
}

impl IndexMethod for HashMethod {
    fn name(&self) -> &'static str {
        HASH_METHOD
    }
    fn attach(&self, host: &Host) -> Result<()> {
        self.use_budget(Arc::clone(&host.memory));
        Ok(())
    }
    fn supports(&self, op: CompOp) -> bool {
        op == CompOp::Eq
    }
    /// The whole key's equality only.
    fn supports_prefix(&self, columns: usize, equal: usize, then: Option<CompOp>) -> bool {
        equal == columns && then.is_none()
    }
    fn create(&self, columns: &[DataType]) -> Result<Box<dyn Index>> {
        let (pool, file) = self.files()?;
        Ok(self.index((HashTable::create(pool, file)?, KeyCounts::new()), columns))
    }
    /// The index whose table's meta page the locator names, counting the
    /// entries and the keys it holds (`HashIndex::locator`).
    fn open(&self, columns: &[DataType], locator: &[u8]) -> Result<Option<Box<dyn Index>>> {
        let u64_at = |at: usize| locator.get(at..at.saturating_add(8)).and_then(|b| b.try_into().ok()).map(u64::from_le_bytes);
        let (Some(entries), Some(bytes), Some(located)) = (u64_at(0), u64_at(8), locator.get(16..)) else {
            return Err(SqlError::new(Code::Corrupt, "[DESIGN] a hash index's locator is malformed".to_owned()));
        };
        let (meta, keys) = KeyCounts::located(located)?;
        let (pool, file) = self.files()?;
        let table = HashTable::open(pool, file, PageId(meta))?.counted((entries, bytes));
        Ok(Some(self.index((table, keys), columns)))
    }
    /// Writes each index's counts, then writes back every page and syncs
    /// the file.
    fn checkpoint(&self) -> Result<()> {
        let tables: Vec<Arc<HashTable>> = lock_builtin(&self.tables).iter().filter_map(Weak::upgrade).collect();
        tables.iter().try_for_each(|t| t.persist())?;
        let (pool, file) = self.files()?;
        pool.sync(file, Durability::Full)
    }
}

fn corrupt() -> SqlError {
    SqlError::new(Code::Corrupt, "[DESIGN] a hash index entry is malformed")
}

/// The row of an entry's value.
fn row_of(value: &[u8]) -> Result<RowId> {
    <[u8; ROW_ID]>::try_from(value).map(RowId::from_be_bytes).map_err(|_| corrupt())
}

/// `rows`, each once, in the order first found: those seen kept in a set,
/// so a key of many rows costs no more than sorting them.
fn once_each(rows: Vec<RowId>) -> Vec<RowId> {
    let mut seen = BTreeSet::new();
    rows.into_iter().filter(|&r| seen.insert(r)).collect()
}

/// An index of the method: its table, its key's types, and whether it was
/// dropped, so its pages are freed when it goes.
struct HashIndex {
    table: Arc<HashTable>,
    types: Vec<DataType>,
    discarded: AtomicBool,
    /// Its distinct keys, counted as written, or by maintenance once asked
    /// for (`index/stats.rs`).
    keys: KeyCounts,
}

/// A dropped index's pages are freed when the last reference to it goes,
/// so no reader of an earlier snapshot reads a page given to another use.
/// A failure cannot be reported from here: the pages are then lost to the
/// file until it is rebuilt, which costs space only.
impl Drop for HashIndex {
    fn drop(&mut self) {
        if self.discarded.load(Ordering::Acquire) {
            let _ = self.table.free();
        }
    }
}

/// Where a probe looks: under one key, its entries checked against it if
/// `checked`; at every entry, each checked; or nowhere.
enum Lookup {
    Key { key: Vec<u8>, checked: bool },
    Every,
    Nothing,
}

impl HashIndex {
    /// The encoded key of `key`.
    fn encoded(&self, key: &Key) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        encode_key(&key.0, &self.types, &mut out)?;
        if out.len().saturating_add(ROW_ID) > self.table.max_entry() {
            return Err(SqlError::new(
                Code::Unsupported,
                format!(
                    "[DESIGN] an index entry of {} bytes is longer than the limit of {} bytes for this database's page size; a database created with a larger STORAGE.PAGE_BYTES holds longer entries",
                    out.len(),
                    self.table.max_entry()
                ),
            ));
        }
        Ok(out)
    }

    /// Where `probe` looks: the key of its values, each placed by its
    /// anchor; nowhere for a comparison with NULL, which is never true
    /// [X3.135-1986 §5.11 GR2].
    fn lookup(&self, probe: &Probe) -> Lookup {
        let values: &[Value] = match probe {
            Probe::All => return Lookup::Every,
            Probe::Compare { value, .. } => std::slice::from_ref(value),
            Probe::Prefix { equal, .. } | Probe::Range { equal, .. } => equal,
        };
        if values.iter().any(Value::is_null) {
            return Lookup::Nothing;
        }
        let (mut key, mut checked) = (Vec::new(), false);
        for (value, &data_type) in values.iter().zip(&self.types) {
            let Some(at) = anchor(value, data_type) else { return Lookup::Every };
            checked |= !at.exact;
            key.extend(at.key);
        }
        Lookup::Key { key, checked }
    }

    /// The (key, row) pairs under `key`, each once, its key decoded: none
    /// if `checked` and the key does not answer `probe`.
    fn keyed(&self, key: &[u8], checked: bool, probe: &Probe) -> Result<Vec<(Key, RowId)>> {
        let rows = once_each(self.table.get(key)?.iter().map(|v| row_of(v)).collect::<Result<_>>()?);
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let decoded = Key(decode_key(key, &self.types)?);
        if checked && !probe.answers(&decoded.0) {
            return Ok(Vec::new());
        }
        Ok(rows.into_iter().map(|r| (decoded.clone(), r)).collect())
    }
}

/// The (key, row) pairs of `bucket` that answer `probe`, each once (a
/// pair's entries are all in its key's bucket), their keys decoded as
/// `types` say.
fn answering(bucket: Vec<(Vec<u8>, Vec<u8>)>, types: &[DataType], probe: &Probe) -> Result<Vec<(Key, RowId)>> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for (k, v) in bucket {
        let row = row_of(&v)?;
        if !seen.insert((row, k.clone())) {
            continue;
        }
        let key = Key(decode_key(&k, types)?);
        // Every entry, unchecked, only for `Probe::All`.
        if matches!(probe, Probe::All) || probe.answers(&key.0) {
            out.push((key, row));
        }
    }
    Ok(out)
}

impl Index for HashIndex {
    fn insert(&self, key: Key, row: RowId) -> Result<()> {
        let encoded = self.encoded(&key)?;
        self.keys.added(&encoded);
        self.table.insert(&encoded, &row.to_be_bytes())
    }

    fn remove(&self, key: &Key, row: RowId) -> Result<()> {
        self.keys.removed();
        self.table.remove(&self.encoded(key)?, &row.to_be_bytes()).map(|_| ())
    }

    /// Every entry's key, counted: of an index opened, once maintenance
    /// has made its pass (`index/stats.rs`).
    fn distinct_keys(&self) -> Option<f64> {
        self.keys.distinct()
    }

    /// A pass over its keys, once asked for.
    fn maintenance_due(&self) -> bool {
        self.keys.due()
    }

    /// The pass over every entry's key as encoded, if due: equal keys are
    /// equal bytes, as the table finds them by.
    fn maintain(&self) -> Result<bool> {
        self.keys.count(|| Ok(Box::new(self.table.keys())))
    }

    /// A lookup reads its key's bucket; a probe of every entry, each bucket.
    fn probe_pages(&self, probe: &Probe) -> Option<f64> {
        match self.lookup(probe) {
            Lookup::Nothing => Some(0.0_f64),
            Lookup::Key { .. } => Some(1.0_f64),
            Lookup::Every => u32::try_from(self.table.buckets()).ok().map(f64::from),
        }
    }

    /// The key's entries, in its bucket's chain, read whole: as
    /// `key_rows` gives them too, by default.
    fn rows(&self, key: &Key) -> Result<Vec<RowId>> {
        let found = self.table.get(&self.encoded(key)?)?;
        Ok(once_each(found.iter().map(|v| row_of(v)).collect::<Result<_>>()?))
    }

    /// Exact for one key, a lookup; every entry's count for every row,
    /// pairs inserted twice counted twice; not known for a probe that
    /// checks every entry.
    fn estimate(&self, probe: &Probe) -> Option<usize> {
        match self.lookup(probe) {
            Lookup::Nothing => Some(0),
            Lookup::Key { key, checked: false } => {
                self.table.get(&key).ok().map(|v| once_each(v.iter().filter_map(|v| row_of(v).ok()).collect()).len())
            }
            Lookup::Key { key, checked: true } => self.keyed(&key, true, probe).ok().map(|p| p.len()),
            // Only `Probe::All` is answered by every entry unchecked.
            Lookup::Every => matches!(probe, Probe::All).then(|| usize::try_from(self.table.len()).ok()).flatten(),
        }
    }

    /// One key's entries, a lookup; or every entry, a bucket at a time,
    /// the table's shape held meanwhile (`engine/hash/scan.rs`). In no
    /// order: `order` is not asked of a method not ordered.
    fn search(self: Arc<Self>, probe: &Probe, _order: KeyOrder) -> Entries {
        match self.lookup(probe) {
            Lookup::Nothing => Box::new(std::iter::empty()),
            Lookup::Key { key, checked } => match self.keyed(&key, checked, probe) {
                Ok(found) => Box::new(found.into_iter().map(Ok)),
                Err(e) => Box::new(std::iter::once(Err(e))),
            },
            Lookup::Every => {
                let (types, probe) = (self.types.clone(), probe.clone());
                let buckets = Arc::clone(&self.table).scan();
                Box::new(buckets.flat_map(move |bucket| match bucket.and_then(|b| answering(b, &types, &probe)) {
                    Ok(found) => found.into_iter().map(Ok).collect::<Vec<_>>(),
                    Err(e) => vec![Err(e)],
                }))
            }
        }
    }

    /// Its table's entries and their bytes, u64 each, little-endian; then
    /// its meta page and its key counts (`index/stats.rs`). A checkpoint
    /// takes it with its pages, under the gate, so an index opened from the
    /// checkpoint counts what those pages hold: its meta page's counts are
    /// written only as it grows or shrinks, and at a close.
    fn locator(&self) -> Vec<u8> {
        let (entries, bytes) = self.table.counts();
        [entries.to_le_bytes().as_slice(), &bytes.to_le_bytes(), &self.keys.locator(self.table.meta().0)].concat()
    }

    fn discard(&self) {
        self.discarded.store(true, Ordering::Release);
    }
}

// Unit tests: equality probes with values of the column's type and not,
// estimates, counted pairs, reopening and freeing (RULES.md R-TEST-1,
// R-TEST-2).
#[cfg(test)]
mod tests;
