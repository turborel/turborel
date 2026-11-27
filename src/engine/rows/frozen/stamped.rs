//! \[EXTENSION\] Groups a load writes straight from its rows (RULES.md
//! D533). A column store table that holds no row past its last group as a
//! load into it begins takes the load's batches as groups, never through
//! its tree; loads at once each write their own (RULES.md D1142). Such a group is stamped with who made
//! it: its transaction while open, seen by it alone; its commit timestamp
//! once committed, seen by each snapshot taken at or after it; and none
//! once committed before every snapshot, as every other group is. A
//! directory notes whether any group is stamped, so a reader of one that
//! holds none takes it as it is, by one comparison; else it takes a view
//! of the groups it sees. A group is undone with its transaction's other
//! changes, taken out of the directory (`mvcc.rs`).
//!
//! A directory stored names the transaction of each group made by one
//! open then. Opened again after a crash, the table holds such groups
//! apart, and recovery, knowing which transactions committed, keeps each
//! whose rows it neither redoes nor undoes, committed before it, and
//! drops the others before it redoes and undoes rows; their rows then
//! come back by redo only if their transaction committed.
//!
//! ```text
//! directory:  count u32, the high bit set if stamps follow; each group's
//!             stamp u64, its transaction with the high bit set, else 0;
//!             each group's summary (`group/header.rs`)
//! ```

use crate::engine::rows::options::{Columns, LoadTo};
use std::mem;
use std::ops::Bound;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::freeze::ordered;
use super::records::{Batch, group_of_records};
use super::{Directory, Entry};
use crate::engine::columns::group::{Group, Pages};
use crate::engine::rows::mvcc::Reader;
use crate::engine::rows::sealing::Sealing;
use crate::engine::rows::version::{Maker, Version};
use crate::engine::rows::{RowTable, corrupt, key};
use crate::engine::tree::Direction;
use crate::error::Result;
use crate::storage::lanes::Records;
use crate::storage::{Loaded, Loading, Newest, Row, RowId};
use crate::sync::{lock_builtin, read, write};
use crate::txn::{NO_TRANSACTION, Timestamp, TxId};

/// The high bit of a stamp: the group's transaction is the rest, open.
const OPEN: u64 = 1 << 63;
/// The high bit of a stored directory's count: stamps follow it.
const STAMPED: u32 = 1 << 31;

/// A table's loads straight to groups.
#[derive(Default)]
pub(in crate::engine::rows) struct Loads {
    /// The transactions whose rows go straight to groups, each writing its
    /// own (RULES.md D1142).
    direct: std::sync::Mutex<Vec<TxId>>,
    /// How many `direct` holds, so an insert or a transaction's end with
    /// no load under way reads this word alone.
    loading: std::sync::atomic::AtomicUsize,
    /// Groups stored as made by a transaction open then, held apart until
    /// recovery settles them.
    doubtful: std::sync::Mutex<Vec<Arc<Entry>>>,
    /// The segment each loading stream appends its groups to (RULES.md
    /// D1146, `loaded.rs`).
    pub(super) streams: std::sync::Mutex<Vec<(TxId, Arc<super::extents::Segment>)>>,
}

impl Entry {
    /// Its stamp: 0 if committed before every snapshot.
    pub(super) fn stamp(&self) -> u64 {
        self.made.load(Ordering::Acquire)
    }
}

impl Directory {
    /// A directory of `groups`, noting whether any is stamped.
    pub(super) fn of(groups: Vec<Arc<Entry>>) -> Directory {
        let pending = groups.iter().any(|e| e.stamp() != 0);
        Directory { groups, pending, _whole: None }
    }

    /// `groups`, a part of `whole`'s that a reader sees, held with it.
    pub(in crate::engine::rows) fn part(whole: &Arc<Directory>, groups: Vec<Arc<Entry>>) -> Directory {
        Directory { groups, pending: false, _whole: Some(Arc::clone(whole)) }
    }
}

/// The count of a stored `directory`, and the stamps of its groups made
/// by a transaction open, if any is.
pub(super) fn count_of(directory: &Directory) -> Result<Vec<u8>> {
    let count = u32::try_from(directory.groups.len()).ok().filter(|c| c & STAMPED == 0);
    let count = count.ok_or_else(|| corrupt("it has too many groups"))?;
    let open: Vec<u64> = directory.groups.iter().map(|e| e.stamp()).map(|s| if s & OPEN == 0 { 0 } else { s }).collect();
    if open.iter().all(|&s| s == 0) {
        return Ok(count.to_le_bytes().to_vec());
    }
    let mut bytes = (count | STAMPED).to_le_bytes().to_vec();
    for stamp in open {
        bytes.extend_from_slice(&stamp.to_le_bytes());
    }
    Ok(bytes)
}

/// The count `count_of` wrote, the stamps if any, and the bytes after.
pub(super) fn counted(bytes: &[u8]) -> Result<(usize, Vec<u64>, &[u8])> {
    let (count, mut rest) = bytes.split_first_chunk::<4>().ok_or_else(|| corrupt("its groups' directory is too short"))?;
    let count = u32::from_le_bytes(*count);
    let groups = usize::try_from(count & !STAMPED).map_err(|_| corrupt("its groups' directory is too long"))?;
    let mut stamps = Vec::new();
    if count & STAMPED != 0 {
        for _ in 0..groups {
            let (stamp, after) = rest.split_first_chunk::<8>().ok_or_else(|| corrupt("its groups' stamps are not whole"))?;
            stamps.push(u64::from_le_bytes(*stamp));
            rest = after;
        }
    }
    Ok((groups, stamps, rest))
}

impl RowTable {
    /// Of `groups`, stored with `stamps`: those made by a transaction open
    /// then held apart for recovery to settle, the others given.
    pub(super) fn unstamped(&self, groups: Vec<Arc<Entry>>, stamps: &[u64]) -> Vec<Arc<Entry>> {
        if stamps.is_empty() {
            return groups;
        }
        let mut doubtful = lock_builtin(&self.frozen.loads.doubtful);
        let mut kept = Vec::with_capacity(groups.len());
        for (entry, &stamp) in groups.into_iter().zip(stamps) {
            entry.made.store(stamp, Ordering::Release);
            if stamp == 0 { kept.push(entry) } else { doubtful.push(entry) }
        }
        kept
    }

    /// Who made the rows of `entry`: its transaction, looked up in case it
    /// committed since, or the commit that did.
    pub(in crate::engine::rows) fn group_maker(&self, entry: &Entry) -> Maker {
        let stamp = entry.stamp();
        if stamp & OPEN == 0 {
            return Maker::Committed(stamp);
        }
        self.maker(&Version { maker: Maker::Open(stamp & !OPEN), deleted: false, older: None, record: &[] })
    }

    /// \[DESIGN\] The newest state of row `id`, held by no version in the
    /// tree, for UNIQUE, as of `tx`: its frozen values, pending if a load
    /// of another transaction still open made its group, so a key check
    /// rests on no row that may yet be rolled back (RULES.md D1236).
    pub(in crate::engine::rows) fn frozen_newest(&self, tx: TxId, id: RowId) -> Result<Newest> {
        let directory = self.frozen.directory()?;
        let maker = directory.holding(id).map(|e| self.group_maker(e));
        let row = self.frozen_in(&directory, id, None)?;
        Ok(match maker {
            Some(Maker::Open(by)) if by != tx => Newest { row: None, pending: Some(row), writer: Some(by), stamp: 0 },
            _ => Newest::of(row),
        })
    }

    /// The groups `reader` sees: the directory itself if none is stamped.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if its lock is poisoned.
    pub(in crate::engine::rows) fn directory_for(&self, reader: Reader) -> Result<Arc<Directory>> {
        let directory = self.frozen.directory()?;
        if !directory.pending {
            return Ok(directory);
        }
        let groups = directory.groups.iter().filter(|e| e.stamp() == 0 || reader.sees(self.group_maker(e))).cloned().collect();
        Ok(Arc::new(Directory::part(&directory, groups)))
    }

    /// Row `id`'s frozen values, `columns` of them or all, if a group
    /// `reader` sees holds it.
    pub(in crate::engine::rows) fn frozen_seen(&self, id: RowId, columns: Option<&[usize]>, reader: Reader) -> Result<Option<Row>> {
        let directory = self.frozen.directory()?;
        let seen = directory.holding(id).is_some_and(|e| e.stamp() == 0 || reader.sees(self.group_maker(e)));
        if !seen {
            return Ok(None);
        }
        self.frozen_in(&directory, id, columns)
    }

    /// Stamps the groups `tx` made committed at `at`, its load ended.
    pub(in crate::engine::rows) fn stamp_groups(&self, tx: TxId, at: Timestamp) {
        self.end_direct(tx);
        // A table that never held a group has none of `tx` to stamp.
        if self.frozen.plain() {
            return;
        }
        let Ok(directory) = self.frozen.directory() else { return };
        if directory.pending {
            for e in directory.groups.iter().filter(|e| e.stamp() == OPEN | tx) {
                e.made.store(at, Ordering::Release);
                // Counted as its rows, its making counted as one change
                // already, so statistics are gathered as for the tree's.
                self.gathered.changed(u64::from(e.summary.rows).saturating_sub(1));
            }
        }
    }

    /// Unstamps each group committed at or before `horizon`, so committed
    /// before every snapshot; once none is stamped, the directory is given
    /// again noting so.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if its lock is poisoned.
    pub(in crate::engine::rows) fn settle_groups(&self, horizon: Timestamp) -> Result<()> {
        let directory = self.frozen.directory()?;
        if !directory.pending {
            return Ok(());
        }
        let mut left = false;
        for e in directory.groups.iter().filter(|e| e.stamp() != 0) {
            let settled = match self.group_maker(e) {
                Maker::Committed(at) => at <= horizon,
                Maker::Open(_) => false,
            };
            if settled { e.made.store(0, Ordering::Release) } else { left = true }
        }
        let mut held = write(&self.frozen.directory)?;
        if !left && Arc::ptr_eq(&held, &directory) {
            *held = Arc::new(Directory::of(held.groups.clone()));
        }
        Ok(())
    }

    /// Whether a group is stamped committed, so the maintenance thread
    /// settles it.
    pub(in crate::engine::rows) fn groups_pending(&self) -> bool {
        self.frozen.directory().is_ok_and(|d| d.pending && d.groups.iter().any(|e| e.stamp() != 0 && e.stamp() & OPEN == 0))
    }

    /// Ends the load of `tx` straight to groups, if one is under way: the
    /// count read first, as every insert and every end of a transaction
    /// asks, so with no load under way no lock is taken.
    pub(in crate::engine::rows) fn end_direct(&self, tx: TxId) {
        if tx == NO_TRANSACTION || self.frozen.loads.loading.load(Ordering::Acquire) == 0 {
            return;
        }
        let mut direct = lock_builtin(&self.frozen.loads.direct);
        if let Some(at) = direct.iter().position(|&by| by == tx) {
            direct.swap_remove(at);
            self.frozen.loads.loading.store(direct.len(), Ordering::Release);
        }
        drop(direct);
        self.end_stream(tx);
    }

    /// Whether `tx`'s rows go straight to groups.
    fn loads_directly(&self, tx: TxId) -> bool {
        self.frozen.loads.loading.load(Ordering::Acquire) != 0 && lock_builtin(&self.frozen.loads.direct).contains(&tx)
    }

    /// Takes the group `tx` made holding row `first` out of the directory,
    /// as its change is undone.
    ///
    /// # Errors
    ///
    /// As storing the directory.
    pub(in crate::engine::rows) fn unpublish_group(&self, tx: TxId, first: RowId) -> Result<()> {
        let directory = self.frozen.directory()?;
        let Some(entry) = directory.holding(first).filter(|e| e.stamp() == OPEN | tx) else { return Ok(()) };
        let rows = usize::try_from(entry.summary.rows).unwrap_or(usize::MAX);
        self.publish_entry((Some(entry), None), Vec::new())?;
        self.counted.live.fetch_sub(rows, Ordering::AcqRel);
        Ok(())
    }

    /// The group of `batch`, given `ids`, sorted as `columns` says, written
    /// to `pages`.
    pub(super) fn group_from(&self, pages: &Pages<'_, Sealing>, ids: Vec<RowId>, batch: Batch<'_>, columns: &Columns) -> Result<Group> {
        let (order, bloom, sketch) = (columns.order.places(), columns.bloom.places(), columns.sketch.places());
        let rows = match batch {
            Batch::Rows(rows) => rows,
            Batch::Records(records) => {
                return group_of_records(pages, self.format.types(), (ids, records), (&columns.order, &bloom, &sketch));
            }
        };
        if order.is_empty() {
            return Group::write(pages, self.format.types(), (&ids, rows), (&order, &bloom, &sketch));
        }
        // [DESIGN] RULES.md D1145: the rows sorted where they are, by reference, none copied.
        let (ids, rows) = ordered(&columns.order, ids.into_iter().zip(rows.iter()).collect());
        Group::write(pages, self.format.types(), (&ids, &rows), (&order, &bloom, &sketch))
    }

    /// Writes `batch`, of `tx`, as a group, as `Loading::insert_group` says
    /// of rows and `Loading::insert_records` of rows by column.
    fn insert_batch(&self, tx: TxId, batch: Batch<'_>, whole: bool) -> Result<Option<Loaded>> {
        let direct = self.loads_directly(tx);
        let Some(columns) = (*read(&self.frozen.columns)?).filter(|_| direct && !batch.is_empty()) else { return Ok(None) };
        // One run of ids, so the groups of loads at once never share a range.
        let first = self.next_ids(batch.len())?;
        let ids: Vec<RowId> = (first..).take(batch.len()).collect();
        // [DESIGN] RULES.md D1146: to its stream's segment, where it has one.
        let made = if whole { self.extent_from(tx, (ids.clone(), batch), &columns)? } else { None };
        let (entry, pages, extent, held) = if let Some((mut group, summary, segment, (extent, held))) = made {
            let base = summary.extent.map_or(0, |p| p.base);
            group.place = (segment.file, base);
            let entry = self.entry_in(summary, Some((segment, base)));
            self.headers.put(entry.header_key(), &entry.slot, &Arc::new(group));
            (entry, Vec::new(), Some(extent), held)
        } else {
            let group = self.group_from(&self.group_pages()?, ids.clone(), batch, &columns)?;
            let pages = group.pages();
            (self.entry_of(group)?, pages, None, None)
        };
        entry.made.store(OPEN | tx, Ordering::Release);
        // Noted first, so a group published is undone with the rest.
        if let Err(e) = self.note_group(tx, first) {
            entry.retire(pages);
            return Err(e);
        }
        self.publish_entry((None, Some((entry, pages))), Vec::new())?;
        self.counted.live.fetch_add(batch.len(), Ordering::AcqRel);
        Ok(Some(Loaded { ids, extent, held }))
    }
}

impl Loading for RowTable {
    fn begin_load(&self, tx: TxId, qualifies: bool) -> Result<Option<usize>> {
        let to_groups = |c: &Columns| match c.load {
            LoadTo::Groups => true,
            LoadTo::Default => qualifies,
            LoadTo::Tree => false,
        };
        let Some(columns) = (*read(&self.frozen.columns)?).filter(to_groups) else { return Ok(None) };
        let past = self.frozen.directory()?.last().map(|l| key(l.saturating_add(1)));
        let lower = past.as_ref().map_or(Bound::Unbounded, |k| Bound::Included(&k[..]));
        if self.tree.scan(lower, Bound::Unbounded, Direction::Forward).next().is_some() {
            return Ok(None);
        }
        let mut direct = lock_builtin(&self.frozen.loads.direct);
        if !direct.contains(&tx) {
            direct.push(tx);
            self.frozen.loads.loading.store(direct.len(), Ordering::Release);
        }
        Ok(Some(usize::try_from(columns.group_rows).unwrap_or(usize::MAX)))
    }

    fn insert_group(&self, tx: TxId, rows: &[Row], whole: bool) -> Result<Option<Loaded>> {
        self.insert_batch(tx, Batch::Rows(rows), whole)
    }

    fn insert_records(&self, tx: TxId, columns: &[Records], whole: bool) -> Result<Option<Loaded>> {
        self.insert_batch(tx, Batch::Records(columns), whole)
    }

    fn redo_group(&self, first: RowId, extent: &[u8]) -> Result<()> {
        self.redo_extent(first, extent)
    }

    fn end_load(&self, tx: TxId) {
        self.end_direct(tx);
    }

    fn recovered_loads(&self, redone: &dyn Fn(TxId) -> bool) -> Result<()> {
        let doubtful = mem::take(&mut *lock_builtin(&self.frozen.loads.doubtful));
        if doubtful.is_empty() {
            return Ok(());
        }
        let mut dropped = Vec::new();
        for entry in doubtful {
            if redone(entry.stamp() & !OPEN) {
                dropped.push(entry);
                continue;
            }
            entry.made.store(0, Ordering::Release);
            let pages = self.group_of(&entry)?.pages();
            self.publish_entry((None, Some((entry, pages))), Vec::new())?;
        }
        // Stored without them before their pages are freed.
        self.store_directory(&*self.frozen.directory()?)?;
        for entry in dropped {
            let pages = self.group_of(&entry)?.pages();
            entry.retire(pages);
        }
        self.counted.live.store(self.count_newest()?, Ordering::Release);
        Ok(())
    }
}
