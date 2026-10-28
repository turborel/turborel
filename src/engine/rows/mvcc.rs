//! \[DESIGN\] Multi-version concurrency control in the row store (RULES.md
//! D29, D31). The newest version of each row is in the table's tree, older
//! committed ones in the undo store (`undo.rs`), each version naming the
//! one before it (`version.rs`).
//!
//! * **Reads.** A transaction sees the versions committed at or before its
//!   snapshot, and its own: it goes back from the newest version until it
//!   finds one. Outside any transaction, the newest committed version.
//! * **Writes.** The first change of a transaction to a row appends the
//!   committed version to the undo store and puts the transaction's own
//!   version in the tree, in one compare-and-set, so of two writers only
//!   one succeeds. A row that another open transaction has changed, or
//!   that a transaction committed after this one's snapshot, is refused
//!   (SQLCODE -911): the second writer of a row loses, and no change is
//!   made over one the writer did not see.
//! * **Below SERIALIZABLE** (`TableStorage::read_mode`), a transaction
//!   reads at the snapshot it is told, which moves only forward, and
//!   records what it reads only while told its reads are checked. Its
//!   writes are checked against the snapshot it reads at, so no update is
//!   lost; it is validated as below, from the snapshot of the first read it
//!   recorded, which may refuse a little more than it must.
//! * **Bounds** (RULES.md D38): past TRANSACTIONS.TRACKED_ROWS rows, or
//!   conditions, a transaction's reads count as reading the whole table,
//!   and its writes, validated, as writing every row, so what it keeps to
//!   be checked stays bounded; that may refuse a commit it need not. Its
//!   changes, and a commit's garbage, are lists that spill past
//!   TRANSACTIONS.HELD_BYTES (`lists.rs`); a change keeps the entry it
//!   replaced only if it was its own version, a committed one read back
//!   from the undo store to undo it. A validated transaction's changed
//!   rows are kept to as many bytes, then as each column's least and
//!   greatest value (`reads.rs`). A commit whose changes cannot be read
//!   back stays late: its versions count as committed, never stamped.
//! * **Validation** is two-sided, under the table's validation lock: a
//!   transaction committing at *c* is refused (SQLCODE -911) if it read a
//!   row that a transaction validated at a timestamp between its snapshot
//!   and *c* wrote, or if it writes a row that a transaction validated
//!   after *c*, which did not see it, read. A scan, or rows found by a
//!   condition through an index, count as reading the whole table, so rows
//!   added meanwhile (phantoms) are caught too. A transaction that wrote
//!   nothing commits at its snapshot and is never checked (`txn.rs`).
//! * **Commit** stamps the transaction's versions with its commit
//!   timestamp. One that cannot be stamped is counted committed all the
//!   same, by a table of late stamps that reads consult, and stamped later.
//! * **Rollback** puts back, newest first, the entries its changes
//!   replaced; a statement's failure does so back to its savepoint. What
//!   an undone change appended to the undo store is kept until every
//!   transaction then open in the table has ended: one may have read the
//!   undone version and be going back from it.
//! * **Garbage collection.** When a commit is past the horizon, the oldest
//!   snapshot open, what it superseded is garbage: the older versions it
//!   appended, the own versions it replaced, and its deletions, which then
//!   leave the tree. Their values out of line are freed, their rows are
//!   returned for their index entries to go (`TableStorage::gc`), and the
//!   undo store is truncated before what is still needed.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};

mod commit;
mod gc;
mod lists;
mod pending;
mod reads;

use lists::{decode_change, encode_change};
pub(in crate::engine::rows) use reads::Read;
use reads::{Changed, Reads};

use super::options::Bounds;
use super::version::{Maker, Version};
use super::{RowTable, key, no_row};
use crate::catalog::TableDesc;
use crate::error::{Code, Result, SqlError};
use crate::hash::murmur::{MurmurMap, MurmurSet};
use crate::log::Lsn;
use crate::spill::{DEFAULT_SPILL_BATCH, ListSpace, SpillList, WorkAreas};
use crate::storage::{Newest, Row, RowId};
use crate::sync::lock_builtin;
use crate::txn::{NO_TRANSACTION, ReadMode, Timestamp, TxId};

fn conflict(id: RowId) -> SqlError {
    SqlError::new(
        Code::Conflict,
        format!("[DESIGN] row {id} was changed by a transaction this one does not see, or is being changed; run the transaction again"),
    )
}

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
    /// version before it was taken out may still go back to it.
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
    /// The earliest timestamp of the garbage, and of the validated, kept
    /// under their locks (0: unknown): a horizon before both has nothing to
    /// collect, which every transaction's end finds without a lock.
    garbage_due: AtomicU64,
    validated_due: AtomicU64,
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
        ListSpace { limit: self.held(), batch: DEFAULT_SPILL_BATCH, areas }
    }
}

impl RowTable {
    /// Who `tx` is as a reader.
    pub(super) fn reader(&self, tx: TxId) -> Result<Reader> {
        if tx == NO_TRANSACTION {
            return Ok(Reader::Latest);
        }
        let open = self.open_of(tx)?;
        let snapshot = lock_builtin(&open).snapshot;
        Ok(Reader::At { tx, snapshot })
    }

    fn open_of(&self, tx: TxId) -> Result<Arc<Mutex<Open>>> {
        let open = self.versions.open.read().unwrap_or_else(std::sync::PoisonError::into_inner);
        open.get(&tx)
            .map(Arc::clone)
            .ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] transaction {tx} has not begun in this table")))
    }

    /// Who made a version, counting a committed transaction whose stamp is
    /// late as committed.
    fn maker(&self, version: &Version<'_>) -> Maker {
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
        let made = || Open { snapshot, changes, reads: Reads::default(), checked: true, reads_since: None };
        open.entry(tx).or_insert_with(|| Arc::new(Mutex::new(made())));
    }

    /// From now, `tx` reads as `mode` says.
    pub(super) fn read_mode_of(&self, tx: TxId, mode: ReadMode) -> Result<()> {
        let open = self.open_of(tx)?;
        let mut open = lock_builtin(&open);
        open.snapshot = open.snapshot.max(mode.snapshot);
        open.checked = mode.checked;
        Ok(())
    }

    /// Records what `tx` read.
    pub(super) fn note_read(&self, tx: TxId, read: Read) -> Result<()> {
        if tx == NO_TRANSACTION {
            return Ok(());
        }
        let open = self.open_of(tx)?;
        let mut open = lock_builtin(&open);
        if !open.checked {
            return Ok(());
        }
        let at = open.snapshot;
        open.reads_since.get_or_insert(at);
        open.reads.add(read, self.versions.tracked());
        Ok(())
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

    /// The record of row `id` that `reader` sees.
    pub(super) fn visible(&self, id: RowId, reader: Reader) -> Result<Option<Vec<u8>>> {
        let Some(newest) = self.tree.get(&key(id))? else { return Ok(None) };
        self.visible_from(newest, reader)
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
    /// deletes it. Outside any transaction, the newest version is replaced
    /// at once, committed, and what it held freed: recovery writes so.
    pub(super) fn write_version(&self, tx: TxId, id: RowId, record: Option<&[u8]>) -> Result<()> {
        if tx == NO_TRANSACTION {
            return self.write_latest(id, record);
        }
        let open = self.open_of(tx)?;
        let snapshot = lock_builtin(&open).snapshot;
        let k = key(id);
        let Some(newest) = self.tree.get(&k)? else { return no_row(id) };
        let (older, appended) = {
            let version = Version::parse(&newest)?;
            match self.maker(&version) {
                Maker::Open(by) => {
                    if by != tx {
                        return Err(conflict(id));
                    }
                    if version.deleted {
                        return no_row(id);
                    }
                    (version.older, None)
                }
                Maker::Committed(at) => {
                    if at > snapshot {
                        return Err(conflict(id));
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
        if !self.tree.replace_if(&k, &newest, &new)? {
            if let Some(at) = appended {
                self.undo.release(at);
            }
            return Err(conflict(id));
        }
        let before = appended.is_none().then_some(newest);
        lock_builtin(&open).changes.push(Change { id, before, appended }, &mut encode_change)?;
        Ok(())
    }

    /// Outside any transaction: replaces row `id`'s newest version with
    /// `record`, committed, or removes it; frees the values it held.
    fn write_latest(&self, id: RowId, record: Option<&[u8]>) -> Result<()> {
        let k = key(id);
        let old = match record {
            Some(record) => self.tree.replace(&k, &Version { maker: Maker::Committed(0), deleted: false, older: None, record }.encode())?,
            None => self.tree.delete(&k)?,
        };
        let Some(old) = old else { return no_row(id) };
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
                self.undo_change(change)?;
            }
        }
        Ok(())
    }

    /// Undoes `change`: the entry it replaced put back, or the row it
    /// added taken out.
    fn undo_change(&self, change: Change) -> Result<()> {
        {
            let k = key(change.id);
            let inserted = change.before.is_none() && change.appended.is_none();
            let before = match (change.before, change.appended) {
                (Some(own), _) => Some(own),
                (None, Some(at)) => Some(self.undo.read(at)?),
                (None, None) => None,
            };
            let replaced = match &before {
                Some(before) => self.tree.replace(&k, before)?,
                None => self.tree.delete(&k)?,
            };
            if let Some(appended) = change.appended {
                self.add_garbage(self.past_open(), vec![Dead::Undone(appended)]);
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

    /// A timestamp past the snapshot of every transaction open in the table:
    /// garbage kept until the horizon reaches it outlives them all.
    fn past_open(&self) -> Timestamp {
        let open = self.versions.open.read().unwrap_or_else(std::sync::PoisonError::into_inner);
        open.values().map(|o| lock_builtin(o).snapshot).max().unwrap_or_default().saturating_add(1)
    }

    /// Ends `tx` without committing it here.
    pub(super) fn end_open(&self, tx: TxId) -> Result<()> {
        let undone = self.undo_changes(tx, 0);
        self.versions.open.write().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&tx);
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
        let Some(newest) = self.tree.get(&key(id))? else { return Ok(Newest { row: None, pending: None }) };
        let version = Version::parse(&newest)?;
        let decoded = |record: &[u8], deleted: bool| if deleted { Ok(None) } else { self.decode(id, record, None).map(Some) };
        let other = match self.maker(&version) {
            Maker::Open(by) => by != tx,
            Maker::Committed(_) => false,
        };
        if !other {
            return Ok(Newest { row: decoded(version.record, version.deleted)?, pending: None });
        }
        let pending = decoded(version.record, version.deleted)?;
        let committed = match version.older {
            Some(at) => self.visible_from(self.undo.read(at)?, Reader::Latest)?,
            None => None,
        };
        let row = committed.map(|record| self.decode(id, &record, None)).transpose()?;
        Ok(Newest { row, pending: Some(pending) })
    }

    /// Every committed version of row `id`, whose newest version is
    /// `newest`, that a reader at or after `horizon` may read.
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
