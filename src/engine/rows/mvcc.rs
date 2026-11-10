//! \[DESIGN\] Multi-version concurrency control in the row store (RULES.md
//! D29, D31).
//!
//! The newest version of each row is in the table's tree. Older committed
//! versions are in the undo store (`undo.rs`), and each version names the
//! one before it (`version.rs`). A transaction sees the versions committed
//! at or before its snapshot, and its own. Outside a transaction, a read
//! sees the newest committed version.
//!
//! A transaction's first change to a row appends the committed version to
//! the undo store and puts its own version in the tree in one
//! compare-and-set, so only one of two writers succeeds. A write is refused
//! (SQLCODE -911) if another open transaction has changed the row, or if a
//! transaction committed a change to it after this one's snapshot.
//!
//! Below SERIALIZABLE (`TableStorage::read_mode`), a transaction reads at
//! the snapshot it is told, which only moves forward, and records its reads
//! only when told they are checked. Its writes are checked against the
//! snapshot it reads at, so no update is lost. It validates from the
//! snapshot of its first recorded read, which may refuse slightly more than
//! it must.
//!
//! Validation runs both ways under the table's validation lock. A
//! transaction committing at *c* is refused (SQLCODE -911) if it read
//! something a transaction validated between its snapshot and *c* wrote. It
//! is also refused if it wrote something read by a transaction validated
//! after *c* from a snapshot before *c*. A row read by id is matched by id.
//! A read by condition, through a filtered scan or an index, is matched
//! against the rows the writer changed, before and after each change, which
//! catches phantoms. A scan with no condition counts as reading the whole
//! table. A transaction that wrote nothing commits at its snapshot and is
//! never checked (`txn.rs`).
//!
//! Commit stamps the transaction's versions with its commit timestamp. A
//! version that can't be stamped still counts as committed, through a table
//! of late stamps that reads consult, and is stamped later.
//!
//! Rollback puts back the entries its changes replaced, newest first. A
//! failed statement does so back to its savepoint. What an undone change
//! appended to the undo store is kept until every other transaction then
//! open in the table has ended, because one may have read the undone
//! version (`gc.rs`).
//!
//! Once a commit is past the horizon, the oldest open snapshot, what it
//! superseded is garbage. Out-of-line values are freed, rows are returned
//! so their index entries can go (`TableStorage::gc`), and the undo store
//! is truncated.
//!
//! What a transaction keeps stays bounded (RULES.md D38). Past
//! TRANSACTIONS.TRACKED_ROWS rows or conditions, its reads count as reading
//! the whole table and its validated writes as writing every row, which may
//! refuse a commit it need not. Its changes and a commit's garbage spill
//! past TRANSACTIONS.HELD_BYTES (`lists.rs`). Changed rows shrink to each
//! column's least and greatest value past the same limit (`reads.rs`). A
//! commit whose changes can't be read back counts as committed and is never
//! stamped.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};

mod commit;
mod gc;
mod lists;
mod open;
mod pending;
mod reads;
mod write;

use lists::{decode_change, encode_change};
pub(in crate::engine::rows) use reads::Read;
use reads::{Changed, Reads};

use super::options::Bounds;
use super::version::{Maker, Version};
use super::{RowTable, key, no_row};
use crate::catalog::TableDesc;
use crate::disk::pager::PageId;
use crate::error::{Code, Result, SqlError};
use crate::hash::murmur::{MurmurMap, MurmurSet};
use crate::log::Lsn;
use crate::spill::{DEFAULT_SPILL_BATCH, ListSpace, SpillList, WorkAreas};
use crate::storage::{Newest, Row, RowId};
use crate::sync::lock_builtin;
use crate::txn::{NO_TRANSACTION, ReadMode, Timestamp, TxId};
use crate::value::types::TypeRegistry;

/// SQLCODE -911: a row another transaction changed. Its row id, of no
/// meaning to whoever reads the message, is not told.
fn conflict() -> SqlError {
    SqlError::new(
        Code::Conflict,
        "[DESIGN] a row it would change was changed by a transaction this one does not see, or is being changed; run the transaction again",
    )
}

/// What a write of a row checks its newest committed version against.
#[derive(Debug, Clone, Copy)]
pub(super) enum Past {
    /// That the transaction's snapshot sees it.
    Snapshot,
    /// That it committed at this timestamp, which `TableStorage::newest`
    /// gave (`TableStorage::write_newest`).
    Stamp(Timestamp),
}

/// A row's newest version, and, if it is one made of its frozen values,
/// the pages of its values out of line (`RowTable::newest_or_frozen`).
type Found = (Vec<u8>, Option<Vec<PageId>>);

/// Who reads.
#[derive(Debug, Clone, Copy)]
pub(super) enum Reader {
    /// A transaction, at its snapshot.
    At { tx: TxId, snapshot: Timestamp },
    /// No transaction: the newest committed versions.
    Latest,
}

impl Reader {
    fn sees(self, maker: Maker) -> bool {
        match (self, maker) {
            (Reader::At { snapshot, .. }, Maker::Committed(at)) => at <= snapshot,
            (Reader::At { tx, .. }, Maker::Open(by)) => by == tx,
            (Reader::Latest, Maker::Committed(_)) => true,
            (Reader::Latest, Maker::Open(_)) => false,
        }
    }
}

/// One change a transaction made to a row, as it is undone.
struct Change {
    id: RowId,
    /// The tree's entry before it, if it was the transaction's own
    /// version; a committed one is the version appended to the undo store,
    /// read back from there; none for a row that was not there.
    before: Option<Vec<u8>>,
    /// Where it appended the committed version it replaced.
    appended: Option<Lsn>,
}

/// A transaction's bookkeeping in a table.
struct Open {
    snapshot: Timestamp,
    changes: SpillList<Change>,
    reads: Reads,
    /// Whether what it reads is recorded, to be checked at commit.
    checked: bool,
    /// The snapshot it read at when it first recorded a read.
    reads_since: Option<Timestamp>,
    /// Whether its writes keep what every snapshot reads
    /// (`TableStorage::preserving`): then no commit is checked against it,
    /// nor it against any.
    preserving: bool,
    /// Whether it has ended here, so a scan keeping it finds it again.
    ended: bool,
}

/// A transaction validated in the table: kept while one could still be
/// ordered against it.
struct Validated {
    tx: TxId,
    snapshot: Timestamp,
    at: Timestamp,
    reads: Reads,
    writes: MurmurSet<RowId>,
    /// Whether it wrote more rows than are tracked, which then count as
    /// every row: its writes and their rows are not kept.
    wrote_many: bool,
    /// What it changed (`reads.rs`). Decoded before the validation lock is
    /// taken, so the lock is held only to compare.
    changed: Changed,
}

impl Validated {
    /// Whether `reads` cover a row this one wrote: one read by id, or with
    /// the whole table, or one whose version before or after this one's
    /// change a condition read matches, though the reader never saw it (a
    /// phantom).
    fn wrote_any(&self, reads: &Reads) -> bool {
        if self.wrote_many {
            return reads.any();
        }
        !self.writes.is_empty()
            && (reads.all || reads.rows.iter().any(|id| self.writes.contains(id)) || self.changed.matched_by(&reads.conditions))
    }
}

/// What a commit made garbage, once it is past the horizon.
#[derive(Clone)]
enum Dead {
    /// A version the transaction made and then replaced itself.
    Own(RowId, Vec<u8>),
    /// The committed version it replaced, in the undo store.
    Older(RowId, Lsn),
    /// A row it deleted, whose deletion then leaves the tree.
    Deletion(RowId),
    /// A committed version appended for a change that was undone: no
    /// version in the tree refers to it, but a reader that read the undone
    /// version before it was taken out may still go back to it. Kept
    /// apart from a commit's garbage, in `Versions::undone`.
    Undone(Lsn),
}

/// Transactions committed whose versions are not all stamped yet.
type Late = MurmurMap<TxId, (Timestamp, Option<Vec<RowId>>)>;

/// \[DESIGN\] The versions bookkeeping of a table.
#[derive(Default)]
pub(super) struct Versions {
    open: RwLock<MurmurMap<TxId, Arc<Mutex<Open>>>>,
    validated: Mutex<Vec<Validated>>,
    /// By commit timestamp, each a list that spills.
    garbage: Mutex<BTreeMap<Timestamp, SpillList<Dead>>>,
    /// The versions undone changes appended (`Dead::Undone`), by the newest
    /// transaction then open in the table besides the one undoing
    /// (`NO_TRANSACTION`: none), each a list that spills: released once
    /// none of those is open.
    undone: Mutex<BTreeMap<TxId, SpillList<Dead>>>,
    /// The earliest timestamp of the garbage, and of the validated, kept
    /// under their locks (0: unknown): a horizon before both has nothing to
    /// collect, which every transaction's end finds without a lock.
    garbage_due: AtomicU64,
    validated_due: AtomicU64,
    /// The latest horizon the table was collected at: no reader is before
    /// it, and what commits at or before it superseded may be gone from
    /// the undo store (`RowTable::unpinned`).
    collected: AtomicU64,
    /// Transactions committed whose versions are not all stamped yet: when
    /// they committed, and the rows left to stamp; none if they could not
    /// be read back, so it is never stamped and stays.
    late: RwLock<Late>,
    /// The most rows of a transaction's reads, or writes, tracked one by
    /// one (TRANSACTIONS.TRACKED_ROWS), and the bytes of its changes held
    /// (TRANSACTIONS.HELD_BYTES); and the store's defaults of them.
    tracked: AtomicUsize,
    held: AtomicUsize,
    base: Bounds,
    /// The database's work areas, once the store is attached.
    work: Arc<RwLock<Option<Arc<WorkAreas>>>>,
}

impl Versions {
    /// The bookkeeping of a table keeping `bounds` of its transactions,
    /// spilling to the work areas of `work`.
    pub(super) fn bounded(bounds: Bounds, work: Arc<RwLock<Option<Arc<WorkAreas>>>>) -> Versions {
        let (tracked, held) = (AtomicUsize::new(bounds.tracked), AtomicUsize::new(bounds.held));
        Versions { tracked, held, base: bounds, work, ..Versions::default() }
    }

    /// Keeps what `desc` sets, else the store's defaults.
    pub(super) fn reconfigure(&self, desc: &TableDesc) {
        let bounds = super::options::bounds_of(desc, self.base);
        self.tracked.store(bounds.tracked, Ordering::Release);
        self.held.store(bounds.held, Ordering::Release);
    }

    fn tracked(&self) -> usize {
        self.tracked.load(Ordering::Acquire)
    }

    fn held(&self) -> usize {
        self.held.load(Ordering::Acquire)
    }

    /// Where a list of its spills.
    fn space(&self) -> ListSpace {
        let areas = self.work.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        // Its lists hold row ids and timestamps, no values.
        ListSpace { limit: self.held(), batch: DEFAULT_SPILL_BATCH, areas, types: TypeRegistry::default() }
    }
}

impl RowTable {
    /// Who made a version, counting a committed transaction whose stamp is
    /// late as committed.
    pub(super) fn maker(&self, version: &Version<'_>) -> Maker {
        match version.maker {
            Maker::Open(by) => {
                let late = self.versions.late.read().unwrap_or_else(std::sync::PoisonError::into_inner);
                late.get(&by).map_or(Maker::Open(by), |&(at, _)| Maker::Committed(at))
            }
            committed @ Maker::Committed(_) => committed,
        }
    }

    pub(super) fn begin_open(&self, tx: TxId, snapshot: Timestamp) {
        let mut open = self.versions.open.write().unwrap_or_else(std::sync::PoisonError::into_inner);
        let changes = SpillList::new(self.versions.space());
        let made =
            || Open { snapshot, changes, reads: Reads::default(), checked: true, reads_since: None, preserving: false, ended: false };
        open.entry(tx).or_insert_with(|| Arc::new(Mutex::new(made())));
    }

    /// From now, `tx` reads as `mode` says.
    pub(super) fn read_mode_of(&self, tx: TxId, mode: ReadMode) -> Result<()> {
        let open = self.open_of(tx)?;
        let mut open = lock_builtin(&open);
        open.snapshot = open.snapshot.max(mode.snapshot);
        open.checked = mode.checked && !open.preserving;
        Ok(())
    }

    /// Marks `tx`'s writes as keeping what every snapshot reads, and its
    /// reads unchecked (`TableStorage::preserving`).
    pub(super) fn preserving_of(&self, tx: TxId) -> Result<()> {
        let open = self.open_of(tx)?;
        let mut open = lock_builtin(&open);
        open.preserving = true;
        open.checked = false;
        open.reads = Reads::default();
        Ok(())
    }

    /// As `visible_from`, of a newest version read into `newest`: its own
    /// record is borrowed, not copied, as it is what most reads see.
    pub(super) fn visible_in<'b>(&self, newest: &'b [u8], reader: Reader) -> Result<Option<Cow<'b, [u8]>>> {
        let version = Version::parse(newest)?;
        if reader.sees(self.maker(&version)) {
            return Ok((!version.deleted).then_some(Cow::Borrowed(version.record)));
        }
        let Some(at) = version.older else { return Ok(None) };
        Ok(self.visible_from(self.undo.read(at)?, reader)?.map(Cow::Owned))
    }

    /// The record of the version of the row whose newest version is
    /// `newest` that `reader` sees, if it sees the row.
    pub(super) fn visible_from(&self, newest: Vec<u8>, reader: Reader) -> Result<Option<Vec<u8>>> {
        let mut bytes = newest;
        loop {
            let older = {
                let version = Version::parse(&bytes)?;
                if reader.sees(self.maker(&version)) {
                    return Ok((!version.deleted).then(|| version.record.to_vec()));
                }
                version.older
            };
            let Some(at) = older else { return Ok(None) };
            bytes = self.undo.read(at)?;
        }
    }
}

impl RowTable {
    /// Inserts row `id`, whose record is `record`, by `tx`.
    pub(super) fn insert_version(&self, tx: TxId, id: RowId, record: &[u8]) -> Result<bool> {
        let maker = if tx == NO_TRANSACTION { Maker::Committed(0) } else { Maker::Open(tx) };
        let version = Version { maker, deleted: false, older: None, record }.encode();
        if !self.tree.insert(&key(id), &version)? {
            return Ok(false);
        }
        if tx != NO_TRANSACTION {
            let open = self.open_of(tx)?;
            lock_builtin(&open).changes.push(Change { id, before: None, appended: None }, &mut encode_change)?;
        }
        Ok(true)
    }

    /// Makes `record` the new version of row `id` by `tx`, or with none,
    /// deletes it: the version it replaced, for its zone. Outside any
    /// transaction, the newest is replaced at once, committed, and what it
    /// held freed, as recovery writes, and none is given back. A write that
    /// finds the entry it read gone or changed is tried once
    /// more: a freeze may have taken out the entry of a row frozen, the same
    /// row then read from its group (`rows/frozen.rs`); else it conflicts.
    pub(super) fn write_version(&self, tx: TxId, id: RowId, record: Option<&[u8]>, past: Past) -> Result<Option<Vec<u8>>> {
        if tx == NO_TRANSACTION {
            return self.write_latest(id, record).map(|()| None);
        }
        if let Some(replaced) = self.try_write(tx, id, record, past)? {
            return Ok(Some(replaced));
        }
        self.try_write(tx, id, record, past)?.map(Some).ok_or_else(conflict)
    }

    /// `write_version`'s work: the version it replaced, none if the entry
    /// it read was gone or changed by then.
    fn try_write(&self, tx: TxId, id: RowId, record: Option<&[u8]>, past: Past) -> Result<Option<Vec<u8>>> {
        let open = self.open_of(tx)?;
        let snapshot = lock_builtin(&open).snapshot;
        let k = key(id);
        let Some((newest, thawed)) = self.newest_or_frozen(id)? else { return no_row(id) };
        let (older, appended) = {
            let version = Version::parse(&newest)?;
            match self.maker(&version) {
                Maker::Open(by) => {
                    if by != tx {
                        return Err(conflict());
                    }
                    if version.deleted {
                        return no_row(id);
                    }
                    (version.older, None)
                }
                Maker::Committed(at) => {
                    let newer = match past {
                        Past::Snapshot => at > snapshot,
                        Past::Stamp(seen) => at != seen,
                    };
                    if newer {
                        return Err(conflict());
                    }
                    if version.deleted {
                        return no_row(id);
                    }
                    let appended = self.undo.append(&newest)?;
                    (Some(appended), Some(appended))
                }
            }
        };
        let new = Version { maker: Maker::Open(tx), deleted: record.is_none(), older, record: record.unwrap_or_default() }.encode();
        let written = if thawed.is_some() { self.tree.insert(&k, &new)? } else { self.tree.replace_if(&k, &newest, &new)? };
        if !written {
            if let Some(at) = appended {
                self.undo.release(at);
            }
            self.free(&thawed.unwrap_or_default());
            return Ok(None);
        }
        if thawed.is_some() {
            self.frozen.shadow();
        }
        let before = appended.is_none().then(|| newest.clone());
        lock_builtin(&open).changes.push(Change { id, before, appended }, &mut encode_change)?;
        Ok(Some(newest))
    }

    /// Row `id`'s newest version: its entry in the tree; else, if it is
    /// frozen, a version holding its frozen values, to change it over, its
    /// entry to be inserted (`rows/frozen.rs`), with the pages of its values
    /// out of line; none if neither.
    fn newest_or_frozen(&self, id: RowId) -> Result<Option<Found>> {
        if let Some(newest) = self.tree.get(&key(id))? {
            return Ok(Some((newest, None)));
        }
        Ok(self.frozen_version(id)?.map(|(version, pages)| (version, Some(pages))))
    }

    /// Outside any transaction: replaces row `id`'s newest version with
    /// `record`, committed, or removes it; frees the values it held. A
    /// frozen row keeps its deletion (`rows/frozen.rs`).
    fn write_latest(&self, id: RowId, record: Option<&[u8]>) -> Result<()> {
        let k = key(id);
        let frozen = self.frozen_holds(id);
        let version =
            Version { maker: Maker::Committed(0), deleted: record.is_none(), older: None, record: record.unwrap_or_default() }.encode();
        let old = if record.is_some() || frozen { self.tree.replace(&k, &version)? } else { self.tree.delete(&k)? };
        let Some(old) = old else {
            if self.frozen_row(id, Some(&[]))?.is_none() {
                return no_row(id);
            }
            self.tree.insert(&k, &version)?;
            self.frozen.shadow();
            return Ok(());
        };
        self.free_values(id, Version::parse(&old)?.record);
        Ok(())
    }

    /// Undoes the changes of `tx` after its first `from`, newest first.
    pub(super) fn undo_changes(&self, tx: TxId, from: usize) -> Result<()> {
        let undone = {
            let open = self.open_of(tx)?;
            let mut open = lock_builtin(&open);
            open.changes.take_from(from, &decode_change)?
        };
        for segment in undone.into_segments(true, &decode_change) {
            for change in segment?.into_iter().rev() {
                self.undo_change(tx, change)?;
            }
        }
        Ok(())
    }

    /// Undoes `change` of `tx`: the entry it replaced put back, or the row
    /// it added taken out.
    fn undo_change(&self, tx: TxId, change: Change) -> Result<()> {
        {
            let k = key(change.id);
            let inserted = change.before.is_none() && change.appended.is_none();
            let before = match (change.before, change.appended) {
                (Some(own), _) => Some(own),
                (None, Some(at)) => Some(self.undo.read(at)?),
                (None, None) => None,
            };
            let replaced = if let Some(before) = &before {
                let replaced = self.tree.replace(&k, before)?;
                // Widened once put back (`rows/zones.rs`).
                self.widen_to(change.id, before)?;
                replaced
            } else {
                let deleted = self.tree.delete(&k)?;
                // Its zone narrowed by the maintenance thread (`rows/zones.rs`).
                self.zones.undone(change.id);
                deleted
            };
            if let Some(appended) = change.appended {
                self.add_undone(tx, appended);
            }
            // What is taken out is the transaction's own version: its values
            // out of line are its own too.
            if let Some(replaced) = replaced {
                let version = Version::parse(&replaced)?;
                self.count_undone(&version, inserted);
                self.free_values(change.id, version.record);
            }
        }
        Ok(())
    }

    /// Ends `tx` without committing it here.
    pub(super) fn end_open(&self, tx: TxId) -> Result<()> {
        let undone = self.undo_changes(tx, 0);
        let removed = self.versions.open.write().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&tx);
        if let Some(open) = removed {
            lock_builtin(&open).ended = true;
        }
        lock_builtin(&self.versions.validated).retain(|v| v.tx != tx);
        undone
    }

    /// How many changes `tx` has made, those from now on begun apart: its
    /// savepoint.
    pub(super) fn changes(&self, tx: TxId) -> usize {
        self.open_of(tx).map_or(0, |open| lock_builtin(&open).changes.mark())
    }
}

impl RowTable {
    /// The newest state of row `id`, for UNIQUE.
    pub(super) fn newest_of(&self, tx: TxId, id: RowId) -> Result<Newest> {
        let Some(newest) = self.tree.get(&key(id))? else { return Ok(Newest::of(self.frozen_row(id, None)?)) };
        let version = Version::parse(&newest)?;
        let decoded = |record: &[u8], deleted: bool| if deleted { Ok(None) } else { self.decode(id, record, None).map(Some) };
        let (writer, stamp) = match self.maker(&version) {
            Maker::Open(by) => (Some(by).filter(|&by| by != tx), 0),
            Maker::Committed(at) => (None, at),
        };
        let Some(by) = writer else {
            return Ok(Newest { row: decoded(version.record, version.deleted)?, pending: None, writer: None, stamp });
        };
        let pending = decoded(version.record, version.deleted)?;
        let committed = match version.older {
            Some(at) => self.visible_from(self.undo.read(at)?, Reader::Latest)?,
            None => None,
        };
        let row = committed.map(|record| self.decode(id, &record, None)).transpose()?;
        Ok(Newest { row, pending: Some(pending), writer: Some(by), stamp: 0 })
    }

    /// Every version of row `id`, whose newest version is `newest`,
    /// committed at or before `as_of`, that a reader at or after `horizon`
    /// may read.
    pub(super) fn versions_of(
        &self,
        id: RowId,
        newest: Vec<u8>,
        (horizon, as_of): (Timestamp, Timestamp),
        columns: &[usize],
    ) -> Result<Vec<(RowId, Row)>> {
        let mut found = Vec::new();
        let mut bytes = newest;
        loop {
            let (older, readable_before) = {
                let version = Version::parse(&bytes)?;
                let maker = self.maker(&version);
                if let (Maker::Committed(at), false) = (maker, version.deleted)
                    && at <= as_of
                {
                    found.push((id, self.decode(id, version.record, Some(columns))?));
                }
                // The version before is read only by snapshots before this
                // one's commit; none is open before the horizon.
                let still = match maker {
                    Maker::Committed(at) => at > horizon,
                    Maker::Open(_) => true,
                };
                (version.older, still)
            };
            match older.filter(|_| readable_before) {
                Some(at) => bytes = self.undo.read(at)?,
                None => return Ok(found),
            }
        }
    }
}
