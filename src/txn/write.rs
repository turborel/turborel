//! The row writes of the data-changing statements [X3.135-1986 §8.4, §8.5,
//! §8.7, §8.11, §8.12]: each changes a row and its index entries together, and
//! logs what undoes it for the coordinator [X3.135-1986 §4.16, §7.3 GR3b i].

use std::borrow::Cow;
use std::sync::Arc;

use super::UndoEntry;
use super::writing::Writing;
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::db::write::system_time::{Changed, Stamp};
use crate::error::{Code, Result, SqlError};
use crate::log::record::{delete_record, update_record};
use crate::storage::{Row, RowId, unchanged};
use crate::txn::{Concurrency, LockWait, Timestamp, TxId, Txn, Undo};
use crate::value::Value;
use escrowed::Guarded;

mod entries;
mod escrowed;
mod noted;

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
    /// [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.11] That a row `write`
    /// gives `table` holds in each registered geometry column a value of
    /// the column's system (`geometry::registered`), and \[EXTENSION\] of a
    /// partition, is one its bounds hold.
    fn check_written(&self, writing: &Writing<'_>, write: &Write<'_>) -> Result<()> {
        // Nothing is registered beside any table: no row is looked at.
        if !writing.checked {
            return Ok(());
        }
        let table = writing.table();
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
        self.update_row_in(tx, &self.writing(table)?, id, row)
    }

    /// As `update_row`, of a row of the table of `writing` (RULES.md D872).
    pub(crate) fn update_row_in(&self, tx: &Txn, writing: &Writing<'_>, id: RowId, row: &Row) -> Result<()> {
        self.change_row(tx, (writing, id), (Write::Update(Cow::Borrowed(row)), false), None).map(|_| ())
    }

    /// [X3.135-1986 §8.4, §8.5] Deletes row `id`, as `update_row` replaces it.
    pub(crate) fn delete_row(&self, tx: &Txn, table: &QualifiedName, id: RowId) -> Result<()> {
        self.delete_row_in(tx, &self.writing(table)?, id)
    }

    /// As `delete_row`, of a row of the table of `writing` (RULES.md D872).
    pub(crate) fn delete_row_in(&self, tx: &Txn, writing: &Writing<'_>, id: RowId) -> Result<()> {
        self.change_row(tx, (writing, id), (Write::Delete, false), None).map(|_| ())
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
        self.rewrite_row_in(tx, (&self.writing(table)?, id), row, recheck)
    }

    /// As `rewrite_row`, of a row of the table of `writing` (RULES.md D872).
    pub(crate) fn rewrite_row_in(
        &self,
        tx: &Txn,
        (writing, id): (&Writing<'_>, RowId),
        row: Option<&Row>,
        recheck: Recheck<'_>,
    ) -> Result<Option<Row>> {
        let write = row.map_or(Write::Delete, |row| Write::Update(Cow::Borrowed(row)));
        self.change_row(tx, (writing, id), (write, false), Some(recheck))
    }

    /// \[DESIGN\] As `rewrite_row` with `row`, the increments reserved on
    /// row `id` applied as its transaction commits (RULES.md D452): not
    /// kept waiting by the reservations of others, whose increments add to
    /// the same columns as its own and are applied over it.
    pub(crate) fn fold_row(&self, tx: &Txn, (table, id): (&QualifiedName, RowId), row: &Row, recheck: Recheck<'_>) -> Result<Option<Row>> {
        self.change_row(tx, (&self.writing(table)?, id), (Write::Update(Cow::Borrowed(row)), true), Some(recheck))
    }

    /// `write` of row `id`, tried again as long as another transaction's
    /// change refuses it and LOCKS.WAIT lets it wait: once that
    /// transaction ends, over the version it committed, if `recheck`
    /// is given and the transaction reads at a snapshot for each statement
    /// [ISO/IEC 9075:1992 §4.28]. The row replaced, none if left alone.
    fn change_row(
        &self,
        tx: &Txn,
        (writing, id): (&Writing<'_>, RowId),
        (mut write, fold): (Write<'_>, bool),
        mut recheck: Option<Recheck<'_>>,
    ) -> Result<Option<Row>> {
        let table = writing.table();
        self.check_written(writing, &write)?;
        self.prepare_in(tx, writing)?;
        // [ISO/IEC 9075-2:2023 §15.14 GR10a] a row of a system-time period
        // stamped, and of a system-versioned table, kept as it was.
        let stamp = if writing.stamped { self.stamp_of(tx, table)? } else { None };
        if let (Some(stamp), Write::Update(row)) = (&stamp, &mut write) {
            stamp.put(row.to_mut());
        }
        // The version it rechecked, and its turn behind the row's earlier
        // waiters (RULES.md D442).
        let (mut seen, mut tries, mut turn): (Option<(Timestamp, Row)>, usize, Option<super::row_waits::Turn>) = (None, 0, None);
        loop {
            self.keep_written(tx, (table, id), &mut write)?;
            let write_once = || {
                // After any wait for a lock: a checkpoint never waits on a waiter.
                let _gate = self.tx.gate.enter();
                Self::write_once(tx, (writing, id), (&write, stamp.as_ref()), seen.as_ref())
            };
            // [DESIGN] RULES.md D452: not over the columns another
            // transaction's increments reserved guard, until it ends.
            let attempt = match self.unreserved(tx, (table, id), (&write, fold), (&mut turn, write_once))? {
                Guarded::Written(attempt) => attempt,
                Guarded::Waited => continue,
            };
            // The turn is kept until the write is made or given up: a
            // write refused once as the waiter's snapshot is past is tried
            // again over the version committed, before the next waiter's.
            let e = match attempt {
                Ok((old, history)) => {
                    // [DESIGN] RULES.md D743: the row's line behind it moves here.
                    super::row_waits::Turn::hand_over(turn.as_ref(), tx, (table, id));
                    return self.written(tx, (old, history), stamp.as_ref()).map(Some);
                }
                Err(e) => e,
            };
            tries = tries.saturating_add(1);
            if !Self::waits(tx, writing, &e, tries) {
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
                        Again::Update(row) => Write::Update(Cow::Owned(row)),
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

    /// Makes each large object of the row `write` gives row `id` of `table`
    /// one the table keeps for it (`keep_lobs`). The row is copied only
    /// then, as an insert's is (`insert_row`), and any other is written as
    /// the statement holds it.
    fn keep_written(&self, tx: &Txn, (table, id): (&QualifiedName, RowId), write: &mut Write<'_>) -> Result<()> {
        let Write::Update(row) = write else { return Ok(()) };
        if !row.iter().any(Value::needs_keeping) {
            return Ok(());
        }
        self.keep_lobs(tx, table, Some(id), row.to_mut())
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
                    // [DESIGN] RULES.md D851: in its turn, overtaken by
                    // a writer that came while it was being woken.
                    *turn = match turn.take() {
                        Some(mine) => tx.overtaken(mine, (table, id), writer)?,
                        None => tx.wait_for_row((table, id), writer)?,
                    };
                }
                next @ (Next::Again | Next::Refused | Next::Over(..)) => return Ok(next),
            }
        }
    }

    /// Whether a writer refused by `e`, after `tries` refusals, waits and
    /// tries the row of the table of `writing` again.
    fn waits(tx: &Txn, writing: &Writing<'_>, e: &SqlError, tries: usize) -> bool {
        let waits = e.code == Code::Conflict && tx.lock_wait == LockWait::Wait && tries < MOST_CONFLICTS;
        waits && writing.caps.concurrency != Concurrency::Coordinator
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
        tx: &Txn,
        (writing, id): (&Writing<'_>, RowId),
        (write, system): (&Write<'_>, Option<&Stamp>),
        seen: Option<&(Timestamp, Row)>,
    ) -> Result<(Row, Option<QualifiedName>)> {
        let (table, storage) = (writing.table(), writing.storage);
        // Borrowed: the engine encodes it, and the caller keeps it for its
        // index entries and its log record, with no copy made (RULES.md D417).
        let new = match write {
            Write::Update(row) => Some(&**row),
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
            Write::Update(row) => Self::updated(tx, (writing, id), (&old, row))?,
            Write::Delete => Self::deleted(tx, writing, id, &old)?,
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

    /// Of row `id` of the table of `writing` made `row` from `old`: what
    /// undoes it, its log record and its index entries.
    fn updated(tx: &Txn, (writing, id): (&Writing<'_>, RowId), (old, row): (&Row, &Row)) -> Result<()> {
        let (table, caps) = (writing.table(), writing.caps);
        if caps.undo == Undo::Log {
            tx.log(UndoEntry::Updated { table: table.clone(), storage: Arc::clone(writing.storage), id, old: old.clone() })?;
        }
        if Self::logs_changes(caps.persistence) {
            Self::log_row(tx, (writing, id), |generation| update_record(tx.id, (table, generation), id, (old, row)))?;
        }
        // An engine keeping versions keeps the old version's entries for
        // the readers that still see it, until the version is removed
        // (`TableStorage::gc`); the new version gets entries of its own, but
        // of an index whose key reads only columns it kept, where the two
        // share one (RULES.md D443).
        let versions = caps.concurrency != Concurrency::Coordinator;
        let kept = if versions { Self::keep_entries(tx, (writing, id), (old, row)) } else { 0 };
        for entries in &writing.indexes {
            // RULES.md D92: an entry of one row's one version keeps
            // its key while the columns its expressions read are unchanged.
            if !versions && entries.ix.has_expressions() && entries.ix.unchanged(old, row) {
                continue;
            }
            if entries.columns != 0 && entries.columns & !kept == 0 {
                continue;
            }
            let entry = Self::entry_id(entries.partition, id)?;
            if !versions {
                Self::index_remove_in(tx, entries, old, entry)?;
            }
            Self::index_add_in(tx, entries, row, entry)?;
        }
        Ok(())
    }

    /// RULES.md D443: of row `id` of the table of `writing` made `row`
    /// from `old` by `tx`, the columns both hold alike of those its indexes
    /// read, noted by its storage on the change; those it noted.
    fn keep_entries(tx: &Txn, (writing, id): (&Writing<'_>, RowId), (old, row): (&Row, &Row)) -> u64 {
        match unchanged(old, row, writing.key_columns) {
            0 => 0,
            kept => writing.storage.keep_entries(tx.id, id, kept),
        }
    }

    /// Of row `id` of the table of `writing`, `old`, deleted, as `updated`
    /// says.
    fn deleted(tx: &Txn, writing: &Writing<'_>, id: RowId, old: &Row) -> Result<()> {
        let (table, caps) = (writing.table(), writing.caps);
        // An engine keeping versions keeps the deleted version's entries
        // until it is removed, as for an update.
        if caps.concurrency == Concurrency::Coordinator {
            for entries in &writing.indexes {
                Self::index_remove_in(tx, entries, old, Self::entry_id(entries.partition, id)?)?;
            }
        }
        if caps.undo == Undo::Log {
            tx.log(UndoEntry::Deleted { table: table.clone(), storage: Arc::clone(writing.storage), id, row: old.clone() })?;
        }
        if Self::logs_changes(caps.persistence) {
            Self::log_row(tx, (writing, id), |generation| delete_record(tx.id, (table, generation), id, old))?;
        }
        Ok(())
    }
}

/// A row's change as a statement writes it: its row borrowed from the
/// statement, and copied only if the write changes it, for a system-time
/// period's stamp, a value the table keeps apart, or a recheck's row.
enum Write<'a> {
    Update(Cow<'a, Row>),
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
