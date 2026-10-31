//! \[DESIGN\] The built-in ordered index, "btree" (RULES.md D31): a B-link
//! tree (`engine/tree.rs`) in a pager file of a buffer pool. An entry's key
//! is the index key encoded so that its bytes sort as SQL compares its
//! values (`engine/key.rs`), then the row id, big-endian: every entry's key
//! is unique, and a key's entries are adjacent, in row id order. Its value
//! is the entry's count (`Index`: one for each version holding it), empty
//! for one, the count's four bytes otherwise, so the usual entry spends
//! nothing on it.
//!
//! Readers lock nothing and writers at most three nodes, as in the tree, so
//! threads search and change an index at once. An entry's count is read
//! and changed under its leaf's lock, in one descent (`BLinkTree::update`),
//! so threads changing one entry at once never lose a change.
//!
//! A probe reads the keys from its value's anchor (`engine/key.rs`) on, or
//! up to it, and checks each key's first value against the probe by SQL's
//! comparison rules, so a value not of the column's type finds exactly the
//! keys it should. A search reads a leaf at a time as its entries are
//! taken (`engine/tree/scan.rs`), forward, or backward for descending keys.
//! An estimate counts the entries a probe selects up to
//! `MOST_COUNTED`: past that, it is not cheap, and the planner estimates from
//! the table's statistics instead.

use std::ops::{Bound, Deref};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::stats::KeyCounts;
use super::{BTREE_METHOD, Entries, Index, IndexMethod, Key, KeyOrder, Probe};
use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::io::Durability;
use crate::disk::pager::PageId;
use crate::engine::key::{anchor, decode_key, encode_key};
use crate::engine::tree::{BLinkTree, Direction, Update, scan_of};
use crate::engine::{MEMORY_POOL_BYTES, MemoryPages};
use crate::error::{Code, Result, SqlError};
use crate::memory::{Host, MemoryBudget};
use crate::spill::{Area, WorkAreas, sorted_records};
use crate::storage::RowId;
use crate::sync::lock_builtin;
use crate::value::{CompOp, DataType, Value};

/// Bytes of a row id at the end of an entry's key.
const ROW_ID: usize = 8;
/// \[DESIGN\] The most entries an estimate counts. Fixed, not a setting
/// (RULES.md D36): what an index method spends on its own estimate, as
/// it states its costs.
const MOST_COUNTED: usize = 1_000;

/// \[DESIGN\] The built-in ordered index method: B-link trees in one pager
/// file.
pub struct BTreeMethod {
    /// Where its indexes are; for a method in memory, made when the first
    /// index is.
    files: Mutex<Option<(Arc<BufferPool>, FileId)>>,
    /// Its pages in memory, for a file made in memory: of a budget of its
    /// own until it is attached to a database's (`attach`).
    memory: Arc<MemoryPages>,
}

impl BTreeMethod {
    /// A method keeping its indexes in pager file `file` of `pool`.
    pub fn new(pool: Arc<BufferPool>, file: FileId) -> BTreeMethod {
        BTreeMethod { files: Mutex::new(Some((pool, file))), memory: MemoryPages::new(0) }
    }

    /// \[DESIGN\] A method keeping its indexes in memory, in a file system of
    /// its own: the built-in method of a database kept nowhere else. Its
    /// file is made when its first index is, so making it cannot fail.
    /// Its pool (`MEMORY_POOL_BYTES`) is reserved then, of the budget of
    /// the database it is attached to, or of its own.
    pub fn in_memory() -> BTreeMethod {
        BTreeMethod::sharing(MemoryPages::new(MEMORY_POOL_BYTES))
    }

    /// A method keeping its indexes in `memory`, which others may share.
    pub(crate) fn sharing(memory: Arc<MemoryPages>) -> BTreeMethod {
        BTreeMethod { files: Mutex::new(None), memory }
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
}

impl IndexMethod for BTreeMethod {
    fn name(&self) -> &'static str {
        BTREE_METHOD
    }
    fn attach(&self, host: &Host) -> Result<()> {
        self.use_budget(Arc::clone(&host.memory));
        Ok(())
    }
    fn supports(&self, op: CompOp) -> bool {
        op != CompOp::Ne
    }
    /// Equal values of one or more first columns, then any operator it
    /// supports on the next.
    fn supports_prefix(&self, columns: usize, equal: usize, then: Option<CompOp>) -> bool {
        equal > 0 && equal.saturating_add(usize::from(then.is_some())) <= columns && then.is_none_or(|op| self.supports(op))
    }
    /// Equal values of any first columns, then two bounds of the next.
    fn supports_range(&self, columns: usize, equal: usize) -> bool {
        equal < columns
    }
    fn ordered(&self) -> bool {
        true
    }
    fn create(&self, columns: &[DataType]) -> Result<Box<dyn Index>> {
        let (pool, file) = self.files()?;
        Ok(Box::new(BLinkIndex::with(BLinkTree::create(pool, file)?, columns, KeyCounts::new())))
    }
    /// The index whose tree's meta page the locator names, with the key
    /// counts it holds.
    fn open(&self, columns: &[DataType], locator: &[u8]) -> Result<Option<Box<dyn Index>>> {
        let (meta, keys) = KeyCounts::located(locator)?;
        let (pool, file) = self.files()?;
        Ok(Some(Box::new(BLinkIndex::with(BLinkTree::open(pool, file, PageId(meta))?, columns, keys))))
    }
    /// Writes back every page and syncs the file.
    fn checkpoint(&self) -> Result<()> {
        let (pool, file) = self.files()?;
        pool.sync(file, Durability::Full)
    }
}

/// An index of the method: its tree, its key's types, and whether it was
/// dropped, so its pages are freed when it goes.
struct BLinkIndex {
    tree: BLinkTree,
    types: Vec<DataType>,
    discarded: AtomicBool,
    /// Its distinct keys, counted as written, or by maintenance once asked
    /// for (`index/stats.rs`).
    keys: KeyCounts,
}

impl BLinkIndex {
    fn with(tree: BLinkTree, columns: &[DataType], keys: KeyCounts) -> BLinkIndex {
        BLinkIndex { tree, types: columns.to_vec(), discarded: AtomicBool::new(false), keys }
    }
}

/// A dropped index's pages are freed when the last reference to it goes,
/// so no reader of an earlier snapshot reads a page given to another use.
/// A failure cannot be reported from here: the pages are then lost to the
/// file until it is rebuilt, which costs space only.
impl Drop for BLinkIndex {
    fn drop(&mut self) {
        if self.discarded.load(Ordering::Acquire) {
            let _ = self.tree.free();
        }
    }
}

fn corrupt() -> SqlError {
    SqlError::new(Code::Corrupt, "[DESIGN] an index entry is malformed")
}

/// The count an entry's value holds.
fn count_of(value: &[u8]) -> Result<u32> {
    if value.is_empty() {
        return Ok(1);
    }
    <[u8; 4]>::try_from(value).map(u32::from_le_bytes).map_err(|_| corrupt())
}

/// The value holding `count`.
fn value_of(count: u32) -> Vec<u8> {
    if count == 1 { Vec::new() } else { count.to_le_bytes().to_vec() }
}

/// The least key past every entry's key beginning with `prefix`: no row id
/// is all 0xFF bytes and one more.
fn past(prefix: &[u8]) -> Vec<u8> {
    let mut end = prefix.to_vec();
    end.resize(prefix.len().saturating_add(ROW_ID).saturating_add(1), u8::MAX);
    end
}

/// The encoded key and the row of the tree's key `entry`.
fn split(entry: &[u8]) -> Result<(&[u8], RowId)> {
    let (key, row) = entry.split_at_checked(entry.len().checked_sub(ROW_ID).ok_or_else(corrupt)?).ok_or_else(corrupt)?;
    Ok((key, <[u8; ROW_ID]>::try_from(row).map(RowId::from_be_bytes).map_err(|_| corrupt())?))
}

/// The least key after every key beginning with `prefix`: its last byte
/// below 0xFF raised by one, the bytes after it dropped; none if every byte
/// is 0xFF, when no key comes after.
fn after(prefix: &[u8]) -> Bound<Vec<u8>> {
    let Some(at) = prefix.iter().rposition(|&b| b < u8::MAX) else { return Bound::Unbounded };
    let mut next = prefix.get(..=at).unwrap_or_default().to_vec();
    if let Some(last) = next.last_mut() {
        *last = last.saturating_add(1);
    }
    Bound::Excluded(next)
}

/// As `after`, as a lower bound: the keys from it on.
fn from_after(prefix: &[u8]) -> Option<Bound<Vec<u8>>> {
    match after(prefix) {
        Bound::Excluded(next) => Some(Bound::Included(next)),
        Bound::Included(_) | Bound::Unbounded => None,
    }
}

/// The bounds of the entries' keys a probe reads, and whether each key
/// read must be checked against the probe: not if the bounds are exact.
struct Range {
    lower: Bound<Vec<u8>>,
    upper: Bound<Vec<u8>>,
    checked: bool,
}

impl Range {
    /// Every entry, each checked if `checked`.
    fn all(checked: bool) -> Range {
        Range { lower: Bound::Unbounded, upper: Bound::Unbounded, checked }
    }
}

impl BLinkIndex {
    /// The encoded key of `key`.
    fn prefix(&self, key: &Key) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        encode_key(&key.0, &self.types, &mut out)?;
        Ok(out)
    }

    /// The tree's key of the entry of `row` under `key`; refused if the
    /// entry would not fit a node.
    fn entry(&self, key: &Key, row: RowId) -> Result<Vec<u8>> {
        let mut entry = self.prefix(key)?;
        entry.extend_from_slice(&row.to_be_bytes());
        let most = self.tree.max_entry();
        if entry.len() > most {
            return Err(SqlError::new(
                Code::Unsupported,
                format!(
                    "[DESIGN] an index entry of {} bytes is longer than the limit of {most} bytes for this database's page size; a database created with a larger STORAGE.PAGE_BYTES holds longer entries",
                    entry.len()
                ),
            ));
        }
        Ok(entry)
    }

    /// The keys `probe` reads, a superset of those it selects, exact if
    /// the probe's values are of their columns' types; none for a
    /// comparison with NULL, which is never true [X3.135-1986 §5.11 GR2].
    fn range(&self, probe: &Probe) -> Option<Range> {
        match probe {
            Probe::All => Some(Range::all(false)),
            Probe::Compare { op, value } => self.prefix_range(&[], Some((*op, value))),
            Probe::Prefix { equal, then } => self.prefix_range(equal, then.as_ref().map(|(op, v)| (*op, v))),
            // From where the lower bound's keys begin to where the upper
            // bound's end.
            Probe::Range { equal, low, high } => {
                let from = self.prefix_range(equal, Some((low.0, &low.1)))?;
                let to = self.prefix_range(equal, Some((high.0, &high.1)))?;
                Some(Range { lower: from.lower, upper: to.upper, checked: from.checked || to.checked })
            }
        }
    }

    /// The keys whose first values are `equal`, then whose next compares
    /// by `then`. Each value is placed by its anchor (`engine/key.rs`): a
    /// value not of its column's type makes the keys read be checked.
    fn prefix_range(&self, equal: &[Value], then: Option<(CompOp, &Value)>) -> Option<Range> {
        if equal.iter().any(Value::is_null) || then.is_some_and(|(_, v)| v.is_null()) {
            return None;
        }
        let (mut prefix, mut checked) = (Vec::new(), false);
        for (value, &data_type) in equal.iter().zip(&self.types) {
            let Some(at) = anchor(value, data_type) else { return Some(Range::all(true)) };
            checked |= !at.exact;
            prefix.extend(at.key);
        }
        let within = |check| Range { lower: Bound::Included(prefix.clone()), upper: after(&prefix), checked: check };
        let Some((op, value)) = then else { return Some(within(checked)) };
        let Some(at) = self.types.get(equal.len()).and_then(|&t| anchor(value, t)) else { return Some(within(true)) };
        let exact = at.exact;
        let key = [prefix.as_slice(), &at.key].concat();
        let (lower, upper) = match op {
            // Exact: the anchor's own entries are those equal to the value.
            CompOp::Eq => (Bound::Included(key.clone()), after(&key)),
            CompOp::Lt if exact => (Bound::Included(prefix.clone()), Bound::Excluded(key)),
            CompOp::Lt | CompOp::Le => (Bound::Included(prefix.clone()), after(&key)),
            CompOp::Gt if exact => (from_after(&key)?, after(&prefix)),
            CompOp::Gt | CompOp::Ge => (Bound::Included(key), after(&prefix)),
            CompOp::Ne => return Some(within(true)),
        };
        Some(Range { lower, upper, checked: checked || !exact })
    }

    /// The entries `probe` selects, read through `tree`, its own, in
    /// `direction`: their keys, decoded if `decoded` or checked, and rows.
    /// What it gives holds `tree` and its own copies of the probe and the
    /// key's types, so it lives as long as `tree` does.
    fn entries<T: Deref<Target = BLinkTree>>(
        &self,
        tree: T,
        probe: &Probe,
        (decoded, direction): (bool, Direction),
    ) -> impl Iterator<Item = Result<(Option<Key>, RowId)>> + use<T> {
        let range = self.range(probe);
        let checked = range.as_ref().is_some_and(|r| r.checked);
        let scan = range.map(|r| scan_of(tree, r.lower.as_ref().map(Vec::as_slice), r.upper.as_ref().map(Vec::as_slice), direction));
        let (probe, types) = (probe.clone(), self.types.clone());
        scan.into_iter().flatten().filter_map(move |entry| {
            let found = entry.and_then(|(entry, _)| {
                let (key, row) = split(&entry)?;
                let key = (decoded || checked).then(|| decode_key(key, &types).map(Key)).transpose()?;
                Ok((key, row))
            });
            match found {
                Ok((key, row)) => (!checked || key.as_ref().is_some_and(|k| probe.answers(&k.0))).then_some(Ok((key, row))),
                Err(e) => Some(Err(e)),
            }
        })
    }
}

/// An index's tree, reached through the index it holds: what a search
/// reads, so the index outlives the search.
struct Held(Arc<BLinkIndex>);

impl Deref for Held {
    type Target = BLinkTree;
    fn deref(&self) -> &BLinkTree {
        &self.0.tree
    }
}

impl Index for BLinkIndex {
    fn insert(&self, key: Key, row: RowId) -> Result<()> {
        self.insert_alone(key, row).map(drop)
    }

    /// Alone if the entries beside its place in its leaf are of other keys
    /// (`BLinkTree::update_alone`): an entry of another row under the key
    /// is beside it, as entries are in key order, then row id.
    fn insert_alone(&self, key: Key, row: RowId) -> Result<bool> {
        let entry = self.entry(&key, row)?;
        // The key's bytes are the same for equal keys (`engine/key.rs`).
        let prefix = entry.len().saturating_sub(ROW_ID);
        self.keys.added(entry.get(..prefix).unwrap_or_default());
        self.tree.update_alone(&entry, prefix, |old| {
            let count = old.map_or(Ok(0), count_of)?;
            Ok(Update::Put(value_of(count.saturating_add(1))))
        })
    }

    /// Built whole: the entries' tree keys sorted in a spill area of `work`,
    /// those alike, of versions of one row under one key, made one entry
    /// counting them, and the tree loaded from them in order
    /// (`BLinkTree::load`).
    fn load(&self, entries: &mut dyn Iterator<Item = Result<(Key, RowId)>>, (areas, area): (&WorkAreas, Area)) -> Result<()> {
        let mut encoded = entries.map(|entry| entry.and_then(|(key, row)| self.entry(&key, row)));
        let mut sorted = sorted_records(areas, area, &mut encoded)?.peekable();
        let mut counted = std::iter::from_fn(|| {
            let first = match sorted.next()? {
                Ok(first) => first,
                Err(e) => return Some(Err(e)),
            };
            let mut count = 1_u32;
            while sorted.next_if(|next| next.as_ref().is_ok_and(|n| *n == first)).is_some() {
                count = count.saturating_add(1);
            }
            for _ in 0..count {
                self.keys.added(first.get(..first.len().saturating_sub(ROW_ID)).unwrap_or_default());
            }
            Some(Ok((first, value_of(count))))
        });
        self.tree.load(&mut counted)
    }

    fn remove(&self, key: &Key, row: RowId) -> Result<()> {
        self.keys.removed();
        self.tree.update(&self.entry(key, row)?, |old| {
            Ok(match old.map(count_of).transpose()? {
                None => Update::Keep,
                Some(0 | 1) => Update::Remove,
                Some(n) => Update::Put(value_of(n.saturating_sub(1))),
            })
        })
    }

    fn rows(&self, key: &Key) -> Result<Vec<RowId>> {
        let prefix = self.prefix(key)?;
        let end = past(&prefix);
        self.tree.scan(Bound::Included(&prefix), Bound::Excluded(&end), Direction::Forward).map(|entry| Ok(split(&entry?.0)?.1)).collect()
    }

    fn estimate(&self, probe: &Probe) -> Option<usize> {
        let mut counted = 0_usize;
        for entry in self.entries(&self.tree, probe, (false, Direction::Forward)) {
            entry.ok()?;
            counted = counted.saturating_add(1);
            if counted > MOST_COUNTED {
                return None;
            }
        }
        Some(counted)
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

    /// The pass over every entry's key, if due.
    fn maintain(&self) -> Result<bool> {
        self.keys.count(|| {
            let all = self.tree.scan(Bound::Unbounded, Bound::Unbounded, Direction::Forward);
            Ok(Box::new(all.map(|e| e.and_then(|(entry, _)| split(&entry).map(|(key, _)| key.to_vec())))))
        })
    }

    /// A descent, its levels the tree's.
    fn probe_pages(&self, _probe: &Probe) -> Option<f64> {
        self.tree.height().ok().map(f64::from)
    }

    fn discard(&self) {
        self.discarded.store(true, Ordering::Release);
    }

    /// Its tree's meta page, and its key counts (`index/stats.rs`).
    fn locator(&self) -> Vec<u8> {
        self.keys.locator(self.tree.meta().0)
    }

    /// Read a leaf at a time (`engine/tree/scan.rs`), backward for
    /// descending keys.
    fn search(self: Arc<Self>, probe: &Probe, order: KeyOrder) -> Entries {
        let direction = match order {
            KeyOrder::Ascending => Direction::Forward,
            KeyOrder::Descending => Direction::Backward,
        };
        let found = self.entries(Held(Arc::clone(&self)), probe, (true, direction));
        Box::new(found.map(|found| found.and_then(|(key, row)| Ok((key.ok_or_else(corrupt)?, row)))))
    }
}

// Unit tests: probes of every operator, with values of the column's type
// and not, counts of entries, estimates, and refusals (RULES.md R-TEST-1,
// R-TEST-2).
#[cfg(test)]
mod tests;
