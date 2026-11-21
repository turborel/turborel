//! The row writes of the data-changing statements [X3.135-1986 §8.4, §8.5,
//! §8.7, §8.11, §8.12]: each changes a row and its index entries together, and
//! logs what undoes it for the coordinator [X3.135-1986 §4.16, §7.3 GR3b i].

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::Arc;

use super::UndoEntry;
use super::kept::key_columns;
use crate::catalog::{IndexDesc, QualifiedName};
use crate::db::State;
use crate::db::write::system_time::{Changed, Stamp};
use crate::error::{Code, Result, SqlError};
use crate::index::{Index, Key};
use crate::log::record::{Change, insert_record};
use crate::storage::{Row, RowId, TableStorage, unchanged};
use crate::txn::{Concurrency, LockWait, Timestamp, TxId, Txn, Undo};
use crate::value::Value;

/// What a writer refused does next (RULES.md D442, D747).
enum Next {
    /// Waits for the open transaction that changed the row.
    Wait(TxId),
    /// Tries its write again.
    Again,
    /// Is refused: the row changed since its snapshot, and it rechecks nothing.
    Refused,
    /// Rechecks its search condition over the row's newest committed
    /// version, of this stamp, none if deleted.
    Over(Timestamp, Option<Row>),
}

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

    /// Logs the insert of `row` as row `id` of `table`: unless its rows are
    /// sealed, made from the row as it is held (RULES.md D531).
    fn log_insert(&self, tx: &Txn, table: &QualifiedName, id: RowId, row: &Row) -> Result<()> {
        let generation = self.generation_of(table);
        if let Some(seal) = self.seal_of(table)? {
            let change = Change::Insert { table: table.clone(), generation, row: id, after: row.clone() };
            return tx.log_sealed(change, Some((&*seal, id)));
        }
        if tx.log.is_none() {
            return Ok(());
        }
        tx.log_encoded(&insert_record(tx.id, (table, generation), id, row), 1)
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
                    // [DESIGN] RULES.md D743: the row's line behind it
                    // queues on this transaction, unwoken.
                    if let Some(turn) = &turn {
                        turn.hand_over(tx, (table, id));
                    }
                    if let (Some(history), Some(stamp)) = (history, &stamp) {
                        self.insert_row(tx, &history, &stamp.ended(&old))?;
                    }
                    return Ok(Some(old));
                }
                Err(e) => e,
            };
            tries = tries.saturating_add(1);
            if !self.waits(tx, table, &e, tries)? {
                return Err(self.refused(tx, table, id, e));
            }
            let check = recheck.as_mut().filter(|_| !tx.one_snapshot());
            let before = seen.take().map(|(at, _)| at);
            match self.next_unwritten(tx, (table, id), check.is_some(), &mut turn)? {
                Next::Wait(_) | Next::Again => {}
                Next::Refused => return Err(e),
                Next::Over(_, None) => return Ok(None),
                Next::Over(committed, Some(row)) => {
                    let Some(check) = check else { return Err(e) };
                    write = match check(&row)? {
                        Again::Update(row) => Write::Update(row),
                        Again::Delete => Write::Delete,
                        Again::Skip => return Ok(None),
                    };
                    // A recheck of a version newer than the last is no
                    // failed try (RULES.md D454).
                    if before != Some(committed) {
                        tries = 0;
                    }
                    seen = Some((committed, row));
                }
            }
        }
    }

    /// \[DESIGN\] RULES.md D454: what a writer refused at row `id` of
    /// `table` does next once no transaction is writing the row, waited
    /// for in `turn` (`next_after`). The row is looked at again after each
    /// wait, so a waiter goes from its wait straight to the version
    /// committed meanwhile.
    fn next_unwritten(
        &self,
        tx: &Txn,
        (table, id): (&QualifiedName, RowId),
        rechecks: bool,
        turn: &mut Option<super::row_waits::Turn>,
    ) -> Result<Next> {
        loop {
            match self.next_after(tx, (table, id), rechecks)? {
                Next::Wait(writer) => {
                    drop(turn.take());
                    *turn = tx.wait_for_row((table, id), writer)?;
                }
                next @ (Next::Again | Next::Refused | Next::Over(..)) => return Ok(next),
            }
        }
    }

    /// Whether a writer refused by `e`, after `tries` refusals, waits and
    /// tries the row of `table` again.
    fn waits(&self, tx: &Txn, table: &QualifiedName, e: &SqlError, tries: usize) -> Result<bool> {
        let waits = e.code == Code::Conflict && tx.lock_wait == LockWait::Wait && tries < MOST_CONFLICTS;
        Ok(waits && self.capabilities(table)?.concurrency != Concurrency::Coordinator)
    }

    /// What a writer refused at row `id` of `table` does next, its writer
    /// and stamp read with no older version (RULES.md D747); the row read
    /// only if it `rechecks`.
    fn next_after(&self, tx: &Txn, (table, id): (&QualifiedName, RowId), rechecks: bool) -> Result<Next> {
        let storage = self.storage_of(table)?;
        let (writer, committed) = storage.newest_writer(tx.id, id)?;
        if let Some(writer) = writer {
            return Ok(Next::Wait(writer));
        }
        // Its writer rolled back as this one looked.
        if committed <= tx.snapshot() {
            return Ok(Next::Again);
        }
        // [§4.28] Committed since its one snapshot: what it read changed.
        // [DESIGN] RULES.md D441: whether rechecked or refused, what it
        // tells rests on that commit.
        tx.depend(committed);
        if !rechecks {
            return Ok(Next::Refused);
        }
        let newest = storage.newest(tx.id, id)?;
        // Changed again as this one looked: the write is tried again.
        if newest.writer.is_some() || newest.stamp != committed {
            return Ok(Next::Again);
        }
        Ok(Next::Over(committed, newest.row))
    }

    /// `e`, the refusal of a write of row `id` of `table`: a conflict is
    /// told once the commit it met, if one did, is durable (RULES.md D441).
    fn refused(&self, tx: &Txn, table: &QualifiedName, id: RowId, e: SqlError) -> SqlError {
        if e.code == Code::Conflict
            && let Ok((_, stamp)) = self.storage_of(table).and_then(|storage| storage.newest_writer(tx.id, id))
        {
            tx.depend(stamp);
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
        let new = match write {
            Write::Update(row) => Some(row.clone()),
            Write::Delete => None,
        };
        // The row replaced, as the write found it, not read before it
        // (RULES.md D417); of a system-time period, read first, so the
        // period refuses the write before it is made.
        let (old, history) = match (seen, system) {
            (Some((stamp, row)), _) => {
                let history = Self::history_of(system, row)?;
                storage.write_newest(tx.id, id, new, *stamp)?;
                (row.clone(), history)
            }
            (None, Some(_)) => {
                let row = storage.get(tx.id, id)?.ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] no row {id} in {table}")))?;
                let history = Self::history_of(system, &row)?;
                (storage.replace(tx.id, id, new)?, history)
            }
            (None, None) => (storage.replace(tx.id, id, new)?, None),
        };
        match write {
            Write::Update(row) => self.updated(tx, (table, id), (&old, row))?,
            Write::Delete => self.deleted(tx, table, id, &old)?,
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
    /// log record and its index entries.
    fn updated(&self, tx: &Txn, (table, id): (&QualifiedName, RowId), (old, row): (&Row, &Row)) -> Result<()> {
        let storage = self.storage_of(table)?;
        let caps = self.capabilities(table)?;
        if caps.undo == Undo::Log {
            tx.log(UndoEntry::Updated { table: table.clone(), storage: Arc::clone(storage), id, old: old.clone() })?;
        }
        if Self::logs_changes(caps.persistence) {
            let change = Change::Update {
                table: table.clone(),
                generation: self.generation_of(table),
                row: id,
                before: old.clone(),
                after: row.clone(),
            };
            tx.log_sealed(change, self.seal_of(table)?.as_deref().map(|p| (p, id)))?;
        }
        // An engine keeping versions keeps the old version's entries for
        // the readers that still see it, until the version is removed
        // (`TableStorage::gc`); the new version gets entries of its own, but
        // of an index whose key reads only columns it kept, where the two
        // share one (RULES.md D443).
        let versions = caps.concurrency != Concurrency::Coordinator;
        let own = |(ix, p): (IndexDesc, Option<u64>)| {
            let columns = if p.is_none() { key_columns(&ix) } else { 0 };
            (ix, p, columns)
        };
        let indexes: Vec<_> = self.table_indexes(table).into_iter().map(own).collect();
        let kept = if versions { Self::keep_entries((tx, storage), id, (old, row), &indexes) } else { 0 };
        for (ix, partition, columns) in indexes {
            // RULES.md D92: an entry of one row's one version keeps
            // its key while the columns its expressions read are unchanged.
            if !versions && ix.has_expressions() && ix.unchanged(old, row) {
                continue;
            }
            if columns != 0 && columns & !kept == 0 {
                continue;
            }
            let entry = Self::entry_id(partition, id)?;
            if !versions {
                self.index_remove(tx, &ix, old, entry)?;
            }
            self.index_add(tx, &ix, row, entry)?;
        }
        Ok(())
    }

    /// RULES.md D443: of row `id` made `row` from `old` by `tx`, the
    /// columns both hold alike of those `indexes` read, noted by `storage`
    /// on the change; those it noted.
    fn keep_entries(
        (tx, storage): (&Txn, &Arc<dyn TableStorage>),
        id: RowId,
        (old, row): (&Row, &Row),
        indexes: &[(IndexDesc, Option<u64>, u64)],
    ) -> u64 {
        let columns = indexes.iter().fold(0, |all, &(_, _, columns)| all | columns);
        match unchanged(old, row, columns) {
            0 => 0,
            kept => storage.keep_entries(tx.id, id, kept),
        }
    }

    /// Of row `id` of `table`, `old`, deleted, as `updated` says.
    fn deleted(&self, tx: &Txn, table: &QualifiedName, id: RowId, old: &Row) -> Result<()> {
        let storage = self.storage_of(table)?;
        let caps = self.capabilities(table)?;
        // An engine keeping versions keeps the deleted version's entries
        // until it is removed, as for an update.
        if caps.concurrency == Concurrency::Coordinator {
            for (ix, partition) in self.table_indexes(table) {
                self.index_remove(tx, &ix, old, Self::entry_id(partition, id)?)?;
            }
        }
        if caps.undo == Undo::Log {
            tx.log(UndoEntry::Deleted { table: table.clone(), storage: Arc::clone(storage), id, row: old.clone() })?;
        }
        if Self::logs_changes(caps.persistence) {
            let change = Change::Delete { table: table.clone(), generation: self.generation_of(table), row: id, before: old.clone() };
            tx.log_sealed(change, self.seal_of(table)?.as_deref().map(|p| (p, id)))?;
        }
        Ok(())
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

/// \[DESIGN\] How many times in a row a row's write is refused by another
/// transaction's change with no recheck of a newer version between,
/// before the statement is: a bound only for
/// an engine whose answers disagree, never for a row in demand, so READ
/// COMMITTED waits until its lock timeout or a deadlock (RULES.md D454).
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
