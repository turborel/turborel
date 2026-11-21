//! The row writes of the data-changing statements [X3.135-1986 §8.4, §8.5,
//! §8.7, §8.11, §8.12]: each changes a row and its index entries together, and
//! logs what undoes it for the coordinator [X3.135-1986 §4.16, §7.3 GR3b i].

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::Arc;

use super::UndoEntry;
use crate::catalog::{IndexDesc, QualifiedName};
use crate::db::State;
use crate::db::write::system_time::{Changed, Stamp};
use crate::error::{Code, Result, SqlError};
use crate::index::{Index, Key};
use crate::log::record::{delete_record, insert_record, update_record};
use crate::log::sealed::seal_plain;
use crate::storage::{Row, RowId, TableStorage};
use crate::txn::{Concurrency, LockWait, Timestamp, TxId, Txn, Undo};
use crate::value::Value;

impl State {
    /// The indexes `table`'s rows have entries in: its own, each entry of
    /// the row's id, and \[EXTENSION\] of a partition, the global indexes
    /// over it, each of the id packed with the partition's (RULES.md D157).
    pub(crate) fn table_indexes(&self, table: &QualifiedName) -> Vec<(IndexDesc, Option<u64>)> {
        let own = self.catalog.indexes(table).into_iter().map(|ix| (ix.clone(), None));
        own.chain(self.globals_over(table).into_iter().map(|(ix, partition)| (ix, Some(partition)))).collect()
    }

    /// The id of the entry of row `id` in an index, packed with `partition`.
    pub(crate) fn entry_id(partition: Option<u64>, id: RowId) -> Result<RowId> {
        partition.map_or(Ok(id), |p| crate::db::partitions::global::packed(p, id))
    }

    /// The key of `row` in the index `ix`: an error an expression of it
    /// raises fails the write (RULES.md D92).
    fn index_key(ix: &IndexDesc, row: &Row) -> Result<Key> {
        ix.computed_key(row)
    }

    pub(crate) fn index_add(&self, tx: &Txn, ix: &IndexDesc, row: &Row, id: RowId) -> Result<()> {
        let key = Self::index_key(ix, row)?;
        if ix.holds(&key) {
            let index = self.index_handle(&ix.name)?;
            // Of a unique index, whether the key's only entry, so its
            // constraint need not look for another row (`Txn::alone`).
            if ix.unique {
                let alone = index.insert_alone(key.clone(), id)?;
                tx.note_alone(&ix.name, id, alone)?;
            } else {
                index.insert(key.clone(), id)?;
            }
            tx.log(UndoEntry::IndexInserted { table: ix.table.clone(), index, key, id })?;
        }
        Ok(())
    }

    pub(crate) fn index_remove(&self, tx: &Txn, ix: &IndexDesc, row: &Row, id: RowId) -> Result<()> {
        let key = Self::index_key(ix, row)?;
        if ix.holds(&key) {
            let index = self.index_handle(&ix.name)?;
            index.remove(&key, id)?;
            tx.log(UndoEntry::IndexRemoved { table: ix.table.clone(), index, key, id })?;
        }
        Ok(())
    }

    /// The index named `name`, shared.
    fn index_handle(&self, name: &str) -> Result<Arc<dyn Index>> {
        self.indexes.get(name).map(Arc::clone).ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] the index {name} does not exist")))
    }

    /// Logs the insert of `row` as row `id` of `table`, made from the row
    /// as it is held, sealed if its rows are (RULES.md D531, D750).
    fn log_insert(&self, tx: &Txn, table: &QualifiedName, id: RowId, row: &Row) -> Result<()> {
        if tx.log.is_none() {
            return Ok(());
        }
        self.log_record(tx, table, id, insert_record(tx.id, (table, self.generation_of(table)), id, row))
    }

    /// [X3.135-1986 §8.7] Inserts `row`, with its index entries, under its
    /// table's lock (`prepare_write`), logging what undoes it [X3.135-1986
    /// §4.16, §7.3 GR3b i].
    pub(crate) fn insert_row(&self, tx: &Txn, table: &QualifiedName, row: &Row) -> Result<RowId> {
        crate::geometry::registered::check_row(&*self.catalog, table, row)?;
        // [EXTENSION] a partition holds only the rows its bounds hold (RULES.md D155).
        if !self.catalog.partitioning().is_empty() {
            self.check_holds(table, row)?;
        }
        self.prepare_write(tx, table)?;
        // A row of large objects is copied, so each names one the table
        // keeps for it, or is held in the row inside another value
        // (RULES.md D79, D106); any other is written as given.
        let kept = if row.iter().any(Value::needs_keeping) {
            let mut kept = row.clone();
            self.keep_lobs(tx, table, None, &mut kept)?;
            Cow::Owned(kept)
        } else {
            Cow::Borrowed(row)
        };
        let kept: &Row = &kept;
        // After any wait for a lock: a checkpoint never waits on a waiter.
        let _gate = self.tx.gate.enter();
        let id = self.storage_of(table)?.insert_from(tx.id, kept)?;
        let caps = self.capabilities(table)?;
        if caps.undo == Undo::Log {
            tx.log(UndoEntry::Inserted { table: table.clone(), storage: Arc::clone(self.storage_of(table)?), id })?;
        }
        if Self::logs_changes(caps.persistence) {
            self.log_insert(tx, table, id, kept)?;
        }
        for (ix, partition) in self.table_indexes(table) {
            self.index_add(tx, &ix, kept, Self::entry_id(partition, id)?)?;
        }
        Ok(id)
    }

    /// [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.11] That a row `write`
    /// gives `table` holds in each registered geometry column a value of
    /// the column's system (`geometry::registered`), and \[EXTENSION\] of a
    /// partition, is one its bounds hold.
    fn check_written(&self, table: &QualifiedName, write: &Write) -> Result<()> {
        match write {
            Write::Update(row) => {
                crate::geometry::registered::check_row(&*self.catalog, table, row)?;
                // [EXTENSION] a row whose key leaves its partition is moved
                // by its statement, never written there (RULES.md D155).
                if self.catalog.partitioning().is_empty() { Ok(()) } else { self.check_holds(table, row) }
            }
            Write::Delete => Ok(()),
        }
    }

    /// [X3.135-1986 §8.11, §8.12] Replaces row `id` with `row`, as `insert_row`
    /// inserts, waiting for a transaction changing it to end (RULES.md D28).
    pub(crate) fn update_row(&self, tx: &Txn, table: &QualifiedName, id: RowId, row: &Row) -> Result<()> {
        self.change_row(tx, (table, id), Write::Update(row.clone()), None).map(|_| ())
    }

    /// [X3.135-1986 §8.4, §8.5] Deletes row `id`, as `update_row` replaces it.
    pub(crate) fn delete_row(&self, tx: &Txn, table: &QualifiedName, id: RowId) -> Result<()> {
        self.change_row(tx, (table, id), Write::Delete, None).map(|_| ())
    }

    /// As `update_row` with `row`, or `delete_row` with none; of a row
    /// another transaction changed after a READ COMMITTED statement's
    /// snapshot, its version committed then written over instead, as
    /// `recheck` says (RULES.md D29): the row it replaced, none if it was
    /// left alone.
    pub(crate) fn rewrite_row(
        &self,
        tx: &Txn,
        (table, id): (&QualifiedName, RowId),
        row: Option<&Row>,
        recheck: Recheck<'_>,
    ) -> Result<Option<Row>> {
        let write = row.map_or(Write::Delete, |row| Write::Update(row.clone()));
        self.change_row(tx, (table, id), write, Some(recheck))
    }

    /// `write` of row `id`, tried again as long as another transaction's
    /// change refuses it and LOCKS.WAIT lets it wait: once that
    /// transaction ends, over the version it committed, if `recheck`
    /// is given and the transaction reads at a snapshot for each statement
    /// [ISO/IEC 9075:1992 §4.28]. The row replaced, none if left alone.
    fn change_row(
        &self,
        tx: &Txn,
        (table, id): (&QualifiedName, RowId),
        mut write: Write,
        mut recheck: Option<Recheck<'_>>,
    ) -> Result<Option<Row>> {
        self.check_written(table, &write)?;
        self.prepare_write(tx, table)?;
        // [ISO/IEC 9075-2:2023 §15.14 GR10a] a row of a system-time period
        // stamped, and of a system-versioned table, kept as it was.
        let stamp = self.stamp_of(tx, table)?;
        if let (Some(stamp), Write::Update(row)) = (&stamp, &mut write) {
            stamp.put(row);
        }
        // The version it rechecked, and its turn behind the row's earlier
        // waiters (RULES.md D442).
        let (mut seen, mut tries, mut turn): (Option<(Timestamp, Row)>, usize, Option<super::row_waits::Turn>) = (None, 0, None);
        loop {
            if let Write::Update(row) = &mut write {
                self.keep_lobs(tx, table, Some(id), row)?;
            }
            let attempt = {
                // After any wait for a lock: a checkpoint never waits on a waiter.
                let _gate = self.tx.gate.enter();
                self.write_once(tx, (table, id), (&write, stamp.as_ref()), seen.as_ref())
            };
            // The turn is kept until the write is made or given up: a
            // write refused once as the waiter's snapshot is past is tried
            // again over the version committed, before the next waiter's.
            let e = match attempt {
                Ok((old, history)) => {
                    if let (Some(history), Some(stamp)) = (history, &stamp) {
                        self.insert_row(tx, &history, &stamp.ended(&old))?;
                    }
                    return Ok(Some(old));
                }
                Err(e) => e,
            };
            tries = tries.saturating_add(1);
            let waits = e.code == Code::Conflict && tx.lock_wait == LockWait::Wait && tries < MOST_CONFLICTS;
            if !waits || self.capabilities(table)?.concurrency == Concurrency::Coordinator {
                return Err(self.refused(tx, table, id, e));
            }
            let newest = self.storage_of(table)?.newest(tx.id, id)?;
            if let Some(writer) = newest.writer {
                drop(turn.take());
                turn = tx.wait_for_row(&format_args!("a row of {table}"), writer)?;
                seen = None;
                continue;
            }
            // Its writer rolled back as this one looked.
            if newest.stamp <= tx.snapshot() {
                seen = None;
                continue;
            }
            // [§4.28] Committed since its one snapshot: what it read changed.
            // [DESIGN] RULES.md D441: whether rechecked or refused, what it
            // tells rests on that commit.
            tx.depend(newest.stamp);
            let Some(check) = recheck.as_mut().filter(|_| !tx.one_snapshot()) else { return Err(e) };
            let Some(row) = newest.row else { return Ok(None) };
            write = match check(&row)? {
                Again::Update(row) => Write::Update(row),
                Again::Delete => Write::Delete,
                Again::Skip => return Ok(None),
            };
            seen = Some((newest.stamp, row));
        }
    }

    /// `e`, the refusal of a write of row `id` of `table`: a conflict is
    /// told once the commit it met, if one did, is durable (RULES.md D441).
    fn refused(&self, tx: &Txn, table: &QualifiedName, id: RowId, e: SqlError) -> SqlError {
        if e.code == Code::Conflict
            && let Ok(newest) = self.storage_of(table).and_then(|storage| storage.newest(tx.id, id))
        {
            tx.depend(newest.stamp);
        }
        e
    }

    /// `write` of row `id` once, over the version `seen` if given, with
    /// its index entries and what undoes it: the row it replaced, and the
    /// history it is kept in, if `stamp`, of the table's system-time
    /// period, says it is [ISO/IEC 9075-2:2023 §15.14 GR10a iii].
    fn write_once(
        &self,
        tx: &Txn,
        (table, id): (&QualifiedName, RowId),
        (write, system): (&Write, Option<&Stamp>),
        seen: Option<&(Timestamp, Row)>,
    ) -> Result<(Row, Option<QualifiedName>)> {
        let storage = self.storage_of(table)?;
        let indexes = self.table_indexes(table);
        // Of an engine sharing entries, the columns their keys read, to
        // learn which the update keeps (RULES.md D750).
        let share = if matches!(write, Write::Update(_)) && storage.sharing().is_some() { key_reads(&indexes) } else { Vec::new() };
        let no_row = || SqlError::new(Code::Api, format!("[DESIGN] no row {id} in {table}"));
        // The row replaced, as the write found it, not read before it
        // (RULES.md D417); of a system-time period, read first, so the
        // period refuses the write before it is made.
        let (old, history, kept) = match (seen, system) {
            (Some((stamp, row)), _) => {
                let history = Self::history_of(system, row)?;
                let (_, kept) = rewrite(&**storage, (tx.id, id), (write, Some(*stamp)), &share)?;
                (row.clone(), history, kept)
            }
            (None, Some(_)) => {
                let row = storage.get(tx.id, id)?.ok_or_else(no_row)?;
                let history = Self::history_of(system, &row)?;
                let (old, kept) = rewrite(&**storage, (tx.id, id), (write, None), &share)?;
                (old.ok_or_else(no_row)?, history, kept)
            }
            (None, None) => {
                let (old, kept) = rewrite(&**storage, (tx.id, id), (write, None), &share)?;
                (old.ok_or_else(no_row)?, None, kept)
            }
        };
        match write {
            Write::Update(row) => self.updated(tx, (table, id), (&old, row), (&indexes, &kept))?,
            Write::Delete => self.deleted(tx, (table, id), &old, &indexes)?,
        }
        Ok((old, history))
    }

    /// The history `old`, a row about to be replaced, is kept in, if
    /// `system`, of its table's system-time period, says it is
    /// [ISO/IEC 9075-2:2023 §15.14 GR10a iii].
    fn history_of(system: Option<&Stamp>, old: &Row) -> Result<Option<QualifiedName>> {
        Ok(match system.map(|s| s.judge(old)).transpose()? {
            Some((Changed::Kept, history)) => history.cloned(),
            Some((Changed::InPlace, _)) | None => None,
        })
    }

    /// Of row `id` of `table` made `row` from `old`: what undoes it, its
    /// log record and its entries in `indexes`, the update having kept the
    /// columns `kept` (RULES.md D750).
    fn updated(
        &self,
        tx: &Txn,
        (table, id): (&QualifiedName, RowId),
        (old, row): (&Row, &Row),
        (indexes, kept): Indexed<'_>,
    ) -> Result<()> {
        let storage = self.storage_of(table)?;
        let caps = self.capabilities(table)?;
        if caps.undo == Undo::Log {
            tx.log(UndoEntry::Updated { table: table.clone(), storage: Arc::clone(storage), id, old: old.clone() })?;
        }
        if Self::logs_changes(caps.persistence) && tx.log.is_some() {
            let record = update_record(tx.id, (table, self.generation_of(table)), id, (old, row));
            self.log_record(tx, table, id, record)?;
        }
        // An engine keeping versions keeps the old version's entries for
        // the readers that still see it, until the version is removed
        // (`TableStorage::gc`); the new version gets entries of its own,
        // but where the old one's serves it, its key's columns kept.
        let versions = caps.concurrency != Concurrency::Coordinator;
        for (ix, partition) in indexes {
            // RULES.md D92: an entry of one row's one version keeps
            // its key while the columns its expressions read are unchanged.
            if !versions && ix.has_expressions() && ix.unchanged(old, row) {
                continue;
            }
            if ix.shares_entry(kept) {
                continue;
            }
            let entry = Self::entry_id(*partition, id)?;
            if !versions {
                self.index_remove(tx, ix, old, entry)?;
            }
            self.index_add(tx, ix, row, entry)?;
        }
        Ok(())
    }

    /// Of row `id` of `table`, `old`, deleted, as `updated` says.
    fn deleted(&self, tx: &Txn, (table, id): (&QualifiedName, RowId), old: &Row, indexes: &[(IndexDesc, Option<u64>)]) -> Result<()> {
        let storage = self.storage_of(table)?;
        let caps = self.capabilities(table)?;
        // An engine keeping versions keeps the deleted version's entries
        // until it is removed, as for an update.
        if caps.concurrency == Concurrency::Coordinator {
            for (ix, partition) in indexes {
                self.index_remove(tx, ix, old, Self::entry_id(*partition, id)?)?;
            }
        }
        if caps.undo == Undo::Log {
            tx.log(UndoEntry::Deleted { table: table.clone(), storage: Arc::clone(storage), id, row: old.clone() })?;
        }
        if Self::logs_changes(caps.persistence) && tx.log.is_some() {
            let record = delete_record(tx.id, (table, self.generation_of(table)), id, old);
            self.log_record(tx, table, id, record)?;
        }
        Ok(())
    }

    /// Logs `record`, of row `id` of `table`, made from the images as the
    /// statement holds them, sealed if its rows are (RULES.md D750).
    fn log_record(&self, tx: &Txn, table: &QualifiedName, id: RowId, record: Vec<u8>) -> Result<()> {
        let record = match self.seal_of(table)? {
            Some(seal) => seal_plain(&record, id, &seal)?,
            None => record,
        };
        tx.log_encoded(record, 1)
    }
}

/// The indexes of a row's table and the columns its update kept.
type Indexed<'a> = (&'a [(IndexDesc, Option<u64>)], &'a [usize]);

/// The columns the keys of `indexes` read, ascending, each once.
fn key_reads(indexes: &[(IndexDesc, Option<u64>)]) -> Vec<usize> {
    let mut reads: Vec<usize> = indexes.iter().flat_map(|(ix, _)| ix.reads()).collect();
    reads.sort_unstable();
    reads.dedup();
    reads
}

/// `write` of row `id`, by `storage`, over the version committed at `seen`
/// if given: the row it replaced, unless `seen`; and of `share`, the
/// columns an engine sharing index entries kept (RULES.md D750).
fn rewrite(
    storage: &dyn TableStorage,
    (tx, id): (TxId, RowId),
    (write, seen): (&Write, Option<Timestamp>),
    share: &[usize],
) -> Result<(Option<Row>, Vec<usize>)> {
    let sharing = storage.sharing().filter(|_| !share.is_empty());
    let new = match (write, sharing) {
        (Write::Update(row), Some(sharing)) => {
            return sharing.share(tx, id, (row.clone(), seen), share).map(|(old, kept)| (Some(old), kept));
        }
        (Write::Update(row), None) => Some(row.clone()),
        (Write::Delete, _) => None,
    };
    match seen {
        Some(stamp) => storage.write_newest(tx, id, new, stamp).map(|()| (None, Vec::new())),
        None => storage.replace(tx, id, new).map(|old| (Some(old), Vec::new())),
    }
}

/// A row's change as a statement writes it.
enum Write {
    Update(Row),
    Delete,
}

/// What a statement does with a row another transaction changed after its
/// snapshot, given the version that transaction committed (RULES.md D29).
pub(crate) enum Again {
    /// Changes it to this row, computed from that version.
    Update(Row),
    /// Deletes it.
    Delete,
    /// Leaves it: its search condition no longer holds of it.
    Skip,
}

/// A statement's check of the version of a row another transaction
/// committed after its snapshot (`State::rewrite_row`).
pub(crate) type Recheck<'a> = &'a mut dyn FnMut(&Row) -> Result<Again>;

/// \[DESIGN\] How many times a row's write is refused by another
/// transaction's change before the statement is, bounding the waits for
/// an engine whose answers disagree.
const MOST_CONFLICTS: usize = 64;

impl Txn {
    /// The row it wrote of form `form`'s change to the kept totals of
    /// `table`, if it wrote one; a statement undone may have taken it.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the transaction's state is poisoned.
    pub(crate) fn kept_row(&self, table: &QualifiedName, form: usize) -> Result<Option<RowId>> {
        Ok(self.state()?.kept_rows.get(&(table.clone(), form)).copied())
    }

    /// Notes `row` as the one it writes of form `form`'s change to the kept
    /// totals of `table`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the transaction's state is poisoned.
    pub(crate) fn note_kept_row(&self, table: &QualifiedName, form: usize, row: RowId) -> Result<()> {
        self.state()?.kept_rows.insert((table.clone(), form), row);
        Ok(())
    }

    /// Notes whether row `row`'s entry in the unique index `index`, as it
    /// was inserted, was its key's only one.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the transaction's state is poisoned.
    pub(crate) fn note_alone(&self, index: &str, row: RowId, alone: bool) -> Result<()> {
        self.note_alone_rows(index, &mut std::iter::once((row, alone)))
    }

    /// As `note_alone` of each row and whether it was alone, of `rows`, the
    /// transaction's state taken once.
    ///
    /// # Errors
    ///
    /// As `note_alone`.
    pub(crate) fn note_alone_rows(&self, index: &str, rows: &mut dyn Iterator<Item = (RowId, bool)>) -> Result<()> {
        let mut state = self.state()?;
        if !state.alone.contains_key(index) {
            state.alone.insert(index.to_owned(), HashSet::new());
        }
        let Some(noted) = state.alone.get_mut(index) else { return Ok(()) };
        for (row, alone) in rows {
            if alone {
                noted.insert(row);
            } else {
                noted.remove(&row);
            }
        }
        Ok(())
    }

    /// Whether row `row`'s entry in the unique index `index`, as it was
    /// last inserted, was its key's only one: another row of its key,
    /// inserted after, was not, and is looked for itself.
    pub(crate) fn alone(&self, index: &str, row: RowId) -> bool {
        self.state().is_ok_and(|s| s.alone.get(index).is_some_and(|rows| rows.contains(&row)))
    }
}
