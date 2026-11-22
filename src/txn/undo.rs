//! What undoes a transaction's changes [X3.135-1986 §8.9 GR1, §7.3 GR3b i]:
//! its participants, and the changes recorded for engines that cannot undo
//! their own and for indexes. Each holds the storage or index it applies
//! to, not its name, so it applies there even after the table was dropped
//! or given a new generation by a change of definitions (`db/ddl.rs`). A
//! transaction holds its entries in memory up to a work area's bytes, then
//! writes them to runs of a spill area (`UndoLog`, RULES.md D38).

use std::sync::Arc;

use std::mem;

use super::escrow::Escrow;
use super::{TxId, Txn, UndoFailure};
use crate::bytes::{Put, Reader};
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::error::{Code, Result, SqlError};
use crate::index::{Index, Key};
use crate::memory::Undoing;
use crate::spill::{ListItem, ListSpace, SpillList, record_row, row_bytes, row_record};
use crate::storage::{Row, RowId, TableStorage};
use crate::value::codec::{put_row, row as read_row};
use crate::value::types::TypeRegistry;

mod loading;

/// A table's storage that has begun a transaction: a participant in its
/// commit and rollback.
#[derive(Clone)]
pub(crate) struct Joined {
    /// The table, for messages and garbage collection: shared, as a
    /// transaction's participants are copied as it reads and ends.
    pub(crate) table: Arc<QualifiedName>,
    /// Its storage when it joined.
    pub(crate) storage: Arc<dyn TableStorage>,
}

impl Joined {
    /// Whether it is `storage`, the same object.
    pub(crate) fn is(&self, storage: &Arc<dyn TableStorage>) -> bool {
        std::ptr::addr_eq(Arc::as_ptr(&self.storage), Arc::as_ptr(storage))
    }
}

/// One change, recorded so that it can be reversed, with the table it was
/// made to and the storage or index it was made in.
#[derive(Clone)]
pub(crate) enum UndoEntry {
    Inserted {
        table: QualifiedName,
        storage: Arc<dyn TableStorage>,
        id: RowId,
    },
    Deleted {
        table: QualifiedName,
        storage: Arc<dyn TableStorage>,
        id: RowId,
        row: Row,
    },
    Updated {
        table: QualifiedName,
        storage: Arc<dyn TableStorage>,
        id: RowId,
        old: Row,
    },
    IndexInserted {
        table: QualifiedName,
        index: Arc<dyn Index>,
        key: Key,
        id: RowId,
    },
    IndexRemoved {
        table: QualifiedName,
        index: Arc<dyn Index>,
        key: Key,
        id: RowId,
    },
    /// \[DESIGN\] The entries of rows inserted together, each as an
    /// `IndexInserted` of its own, in order: one entry a batch, not one a
    /// row (RULES.md D531).
    IndexInsertedRows {
        table: QualifiedName,
        index: Arc<dyn Index>,
        entries: Vec<(Key, RowId)>,
    },
    /// \[DESIGN\] An increment reserved on `column` of row `id`, taken
    /// back (RULES.md D452).
    Escrowed {
        table: QualifiedName,
        escrow: Arc<Escrow>,
        id: RowId,
        column: usize,
    },
    /// \[DESIGN\] Its reservation of `delta` on `column` of row `id`,
    /// guarding `guarded`, applied before its transaction commits, as it
    /// read its own sum: put back (RULES.md D452).
    Folded {
        table: QualifiedName,
        escrow: Arc<Escrow>,
        id: RowId,
        column: usize,
        delta: crate::value::Value,
        guarded: u64,
    },
}

/// [X3.135-1986 §8.9 GR1, §7.3 GR3b i] Reverse `entries`, newest first.
/// Every entry is applied even after one fails, so as much as possible is
/// undone; the failures are returned, with the table each one damaged.
pub(super) fn apply_undo(tx: TxId, entries: Vec<UndoEntry>) -> Vec<UndoFailure> {
    let mut failures = Vec::new();
    for entry in entries.into_iter().rev() {
        let (table, result, rows) = match entry {
            UndoEntry::Inserted { table, storage, id } => (table, storage.delete(tx, id), true),
            UndoEntry::Deleted { table, storage, id, row } => (table, storage.restore(tx, id, row), true),
            UndoEntry::Updated { table, storage, id, old } => (table, storage.update(tx, id, old), true),
            UndoEntry::IndexInserted { table, index, key, id } => (table, index.remove(&key, id), false),
            UndoEntry::IndexRemoved { table, index, key, id } => (table, index.insert(key, id), false),
            UndoEntry::IndexInsertedRows { table, index, entries: made } => (table, removed(&*index, &made), false),
            UndoEntry::Escrowed { table, escrow, id, column } => {
                escrow.take_back(tx, id, column);
                (table, Ok(()), false)
            }
            UndoEntry::Folded { table, escrow, id, column, delta, guarded } => {
                escrow.put_back(id, super::escrow::Reservation { tx, column, delta, guarded });
                (table, Ok(()), false)
            }
        };
        if let Err(error) = result {
            failures.push(UndoFailure { table: Some(table), error, rows });
        }
    }
    failures
}

/// Removes each of `entries` from `index`, newest first, every one tried:
/// the first failure, if one failed.
fn removed(index: &dyn Index, entries: &[(Key, RowId)]) -> Result<()> {
    entries.iter().rev().map(|(key, id)| index.remove(key, *id)).fold(Ok(()), Result::and)
}

/// \[DESIGN\] Where a transaction's undo entries spill (RULES.md D38): the
/// bytes of them it holds in memory, `settings::held_bytes` when it began, and the
/// work areas its runs are made in, with SPILL.BATCH_BYTES's batches.
pub(crate) type UndoSpace = ListSpace;

/// What a spilled entry names, by its place among a log's targets: a
/// table's storage or an index, which a record cannot hold.
#[derive(Clone)]
enum Target {
    Storage(QualifiedName, Arc<dyn TableStorage>),
    Index(QualifiedName, Arc<dyn Index>),
    Escrow(QualifiedName, Arc<Escrow>),
}

impl Target {
    fn table(&self) -> &QualifiedName {
        match self {
            Target::Storage(table, _) | Target::Index(table, _) | Target::Escrow(table, _) => table,
        }
    }

    /// Whether it is the same object as `other`.
    fn is(&self, other: &Target) -> bool {
        match (self, other) {
            (Target::Storage(_, a), Target::Storage(_, b)) => std::ptr::addr_eq(Arc::as_ptr(a), Arc::as_ptr(b)),
            (Target::Index(_, a), Target::Index(_, b)) => std::ptr::addr_eq(Arc::as_ptr(a), Arc::as_ptr(b)),
            (Target::Escrow(_, a), Target::Escrow(_, b)) => Arc::ptr_eq(a, b),
            (Target::Storage(..), Target::Index(..) | Target::Escrow(..))
            | (Target::Index(..), Target::Storage(..) | Target::Escrow(..))
            | (Target::Escrow(..), Target::Storage(..) | Target::Index(..)) => false,
        }
    }
}

impl ListItem for UndoEntry {
    fn bytes(&self) -> usize {
        let values = match self {
            UndoEntry::Inserted { .. } | UndoEntry::Escrowed { .. } => 0,
            UndoEntry::Deleted { row, .. } | UndoEntry::Updated { old: row, .. } => row_bytes(row),
            UndoEntry::Folded { delta, .. } => row_bytes(&vec![delta.clone()]),
            UndoEntry::IndexInserted { key, .. } | UndoEntry::IndexRemoved { key, .. } => row_bytes(&key.0),
            UndoEntry::IndexInsertedRows { entries, .. } => entries
                .iter()
                .fold(0_usize, |sum, (key, _)| sum.saturating_add(row_bytes(&key.0)).saturating_add(size_of::<(Key, RowId)>())),
        };
        size_of::<UndoEntry>().saturating_add(values)
    }
}

/// \[DESIGN\] A transaction's undo entries, oldest first (RULES.md D38), in
/// a list that spills (`spill/list.rs`), each statement's beginning a
/// segment of it: so a transaction that changes many rows holds a work
/// area's bytes of them, and a statement's are undone from where its
/// segment begins, no run read to part them from those before unless they
/// were written in one run with them, as the segments of small statements
/// are. A spilled entry names its storage or index by its place among the
/// log's targets.
#[derive(Default)]
pub(crate) struct UndoLog {
    list: SpillList<UndoEntry>,
    targets: Vec<Target>,
    /// The load under way, whose entries it holds from its mark on.
    loading: Option<loading::Loading>,
}

impl UndoLog {
    /// An empty log, spilling in `space`, shared; its entries may hold
    /// values of the types beyond the standard of the space's registry.
    pub(crate) fn new(space: Arc<UndoSpace>) -> UndoLog {
        UndoLog { list: SpillList::sharing(space), targets: Vec::new(), loading: None }
    }

    /// Begins a segment, as a statement begins: its entries' place.
    pub(crate) fn mark(&mut self) -> usize {
        self.list.mark()
    }

    /// Adds `entry`.
    ///
    /// # Errors
    ///
    /// The spill area's, SQLCODE -918 if it cannot be reserved; the entry
    /// is added all the same, held.
    pub(crate) fn push(&mut self, entry: UndoEntry) -> Result<()> {
        let targets = &mut self.targets;
        self.list.push(entry, &mut |e| encoded(targets, e))
    }

    /// Its entries from the `start`th on, taken from it, to be undone.
    ///
    /// # Errors
    ///
    /// The spill area's, reading a run a place not `mark`'s cuts across:
    /// its entries then all kept.
    pub(crate) fn take_from(&mut self, start: usize) -> Result<UndoLog> {
        let types = self.list.space().types.clone();
        let targets = &self.targets;
        let list = self.list.take_from(start, &|record| entry_of((targets, &types), record))?;
        // A load begun among the entries taken is undone with them.
        if self.loading.as_ref().is_some_and(|l| l.mark >= start) {
            self.loading = None;
        }
        Ok(UndoLog { list, targets: self.targets.clone(), loading: None })
    }

    /// [X3.135-1986 §8.9 GR1, §7.3 GR3b i] Reverses its entries, newest
    /// first, as `apply_undo` does; a run that cannot be read back fails
    /// every table it may have changed.
    pub(crate) fn apply(self, tx: TxId) -> Vec<UndoFailure> {
        let (types, mut failures) = (self.list.space().types.clone(), Vec::new());
        let targets = self.targets;
        for segment in self.list.into_segments(true, &|record| entry_of((&targets, &types), record)) {
            match segment {
                Ok(entries) => failures.extend(apply_undo(tx, entries)),
                Err(error) => {
                    let tables = targets.iter().map(|t| Some(t.table().clone()));
                    failures.extend(tables.map(|table| UndoFailure { table, error: error.clone(), rows: true }));
                }
            }
        }
        failures
    }
}

/// The place of `target` among `targets`, added if new.
fn target(targets: &mut Vec<Target>, target: Target) -> u32 {
    let at = targets.iter().position(|t| t.is(&target)).unwrap_or_else(|| {
        targets.push(target);
        targets.len().saturating_sub(1)
    });
    u32::try_from(at).unwrap_or(u32::MAX)
}

/// `entry` as a record: its kind, its target among `targets`, its row id,
/// then its row or key, if it has one.
fn encoded(targets: &mut Vec<Target>, entry: &UndoEntry) -> Vec<u8> {
    let (kind, named, id, values) = match entry {
        UndoEntry::Inserted { table, storage, id } => (0, Target::Storage(table.clone(), Arc::clone(storage)), *id, None),
        UndoEntry::Deleted { table, storage, id, row } => (1, Target::Storage(table.clone(), Arc::clone(storage)), *id, Some(row)),
        UndoEntry::Updated { table, storage, id, old } => (2, Target::Storage(table.clone(), Arc::clone(storage)), *id, Some(old)),
        UndoEntry::IndexInserted { table, index, key, id } => (3, Target::Index(table.clone(), Arc::clone(index)), *id, Some(&key.0)),
        UndoEntry::IndexRemoved { table, index, key, id } => (4, Target::Index(table.clone(), Arc::clone(index)), *id, Some(&key.0)),
        UndoEntry::IndexInsertedRows { table, index, entries } => {
            return rows_record(target(targets, Target::Index(table.clone(), Arc::clone(index))), entries);
        }
        UndoEntry::Escrowed { table, escrow, id, column } => {
            let mut out = vec![ESCROWED];
            out.put_u32(target(targets, Target::Escrow(table.clone(), Arc::clone(escrow))));
            out.put_u64(*id);
            out.put_u64(u64::try_from(*column).unwrap_or(u64::MAX));
            return out;
        }
        UndoEntry::Folded { table, escrow, id, column, delta, guarded } => {
            let mut out = vec![FOLDED];
            out.put_u32(target(targets, Target::Escrow(table.clone(), Arc::clone(escrow))));
            out.put_u64(*id);
            out.put_u64(u64::try_from(*column).unwrap_or(u64::MAX));
            out.put_u64(*guarded);
            out.extend(row_record(&vec![delta.clone()]));
            return out;
        }
    };
    let mut out = vec![kind];
    out.put_u32(target(targets, named));
    out.put_u64(id);
    out.extend(values.map(row_record).unwrap_or_default());
    out
}

/// The kind of record of an `IndexInsertedRows`.
const INSERTED_ROWS: u8 = 5;
/// The kind of record of an `Escrowed`: its target, its row id, then its
/// column's place.
const ESCROWED: u8 = 6;
/// The kind of record of a `Folded`: its target, its row id, its column's
/// place, the columns it guards, then its increment as a row of one value.
const FOLDED: u8 = 7;

/// The record of an `IndexInsertedRows` of `entries`, of the target at
/// `place`: its kind and target, its entries' count in place of a row id,
/// then each one's row id and key.
fn rows_record(place: u32, entries: &[(Key, RowId)]) -> Vec<u8> {
    let mut out = vec![INSERTED_ROWS];
    out.put_u32(place);
    out.put_u64(u64::try_from(entries.len()).unwrap_or(u64::MAX));
    for (key, id) in entries {
        out.put_u64(*id);
        put_row(&mut out, &key.0);
    }
    out
}

/// The entries `rows_record` wrote, `count` of them, from `rest`.
fn entries_of(rest: &[u8], count: u64, types: &TypeRegistry) -> Result<Vec<(Key, RowId)>> {
    let mut r = Reader::new(rest, "undo entry").with_types(types);
    let mut entries = Vec::new();
    for _ in 0..count {
        let id = r.u64()?;
        entries.push((Key(read_row(&mut r)?), id));
    }
    Ok(entries)
}

/// The entry of `record`.
fn entry_of((targets, types): (&[Target], &TypeRegistry), record: &[u8]) -> Result<UndoEntry> {
    let mut r = Reader::new(record, "undo entry");
    let (kind, target, id) = (r.u8()?, r.u32()?, r.u64()?);
    let rest = r.rest();
    let target = targets.get(usize::try_from(target).unwrap_or(usize::MAX)).ok_or_else(|| r.corrupt("no such target"))?;
    let values = || record_row(rest, types);
    Ok(match (kind, target.clone()) {
        (0, Target::Storage(table, storage)) => UndoEntry::Inserted { table, storage, id },
        (1, Target::Storage(table, storage)) => UndoEntry::Deleted { table, storage, id, row: values()? },
        (2, Target::Storage(table, storage)) => UndoEntry::Updated { table, storage, id, old: values()? },
        (3, Target::Index(table, index)) => UndoEntry::IndexInserted { table, index, key: Key(values()?), id },
        (4, Target::Index(table, index)) => UndoEntry::IndexRemoved { table, index, key: Key(values()?), id },
        (INSERTED_ROWS, Target::Index(table, index)) => {
            UndoEntry::IndexInsertedRows { table, index, entries: entries_of(rest, id, types)? }
        }
        (ESCROWED, Target::Escrow(table, escrow)) => {
            let column = Reader::new(rest, "undo entry").u64()?;
            UndoEntry::Escrowed { table, escrow, id, column: usize::try_from(column).unwrap_or(usize::MAX) }
        }
        (FOLDED, Target::Escrow(table, escrow)) => folded((table, escrow, id), rest, types)?,
        (_, Target::Storage(..) | Target::Index(..) | Target::Escrow(..)) => return Err(r.corrupt("not an entry of its target")),
    })
}

/// The `Folded` of the reservation of row `id` of `table`, kept by
/// `escrow`, whose record ends with `rest`.
fn folded((table, escrow, id): (QualifiedName, Arc<Escrow>, RowId), rest: &[u8], types: &TypeRegistry) -> Result<UndoEntry> {
    let mut r = Reader::new(rest, "undo entry");
    let (column, guarded) = (r.u64()?, r.u64()?);
    let delta = record_row(r.rest(), types)?.into_iter().next().ok_or_else(|| r.corrupt("no increment"))?;
    Ok(UndoEntry::Folded { table, escrow, id, column: usize::try_from(column).unwrap_or(usize::MAX), delta, guarded })
}

impl State {
    /// Marks the tables of `failures` damaged and reports them: `Ok` if there
    /// were none, else one SQLCODE -914 error naming each failure, and `cause`,
    /// what the undo was for.
    pub(super) fn report_damage(&self, failures: &[UndoFailure], cause: &str) -> Result<()> {
        if failures.is_empty() {
            return Ok(());
        }
        let details: Vec<String> = failures
            .iter()
            .map(|f| f.table.as_ref().map_or_else(|| f.error.described(), |table| format!("{table}: {}", f.error.described())))
            .collect();
        // Every table that can be marked is, even if one cannot be found.
        for (lk, rows) in failures.iter().filter_map(|f| Some((self.lock_of(f.table.as_ref()?).ok()?, f.rows))) {
            lk.damaged(rows);
        }
        Err(SqlError::new(
            Code::Damaged,
            format!("[DESIGN] {cause} could not be undone, so these tables are damaged: {}", details.join("; ")),
        ))
    }

    /// [X3.135-1986 §7.3 GR3b i] "All changes made to the database by the
    /// execution of S are canceled." The transaction continues.
    ///
    /// # Errors
    ///
    /// SQLCODE -914 if a change could not be undone; the error names `failure`,
    /// the error that failed the statement.
    pub(crate) fn fail_statement(&self, tx: &Txn, failure: &SqlError) -> Result<()> {
        // Never refused by the memory budget (RULES.md D38).
        let _undoing = Undoing::begin();
        let _gate = self.tx.gate.enter();
        let (entries, savepoints) = {
            let mut st = tx.state()?;
            let start = st.statement_start;
            (st.undo.take_from(start)?, mem::take(&mut st.statement_savepoints))
        };
        let mut failures = entries.apply(tx.id);
        for (joined, sp) in savepoints {
            if let Err(error) = joined.storage.rollback_to(tx.id, sp) {
                failures.push(UndoFailure { table: Some(QualifiedName::clone(&joined.table)), error, rows: true });
            }
        }
        if !failures.is_empty() {
            tx.state()?.doomed = Some("a statement of the transaction could not be undone");
        }
        tx.statement_undone();
        self.report_damage(&failures, &format!("a statement that failed with \"{failure}\""))
    }
}

// Unit tests: entries spilled and read back, a refused area, a run that
// cannot be read, a record not of an entry (RULES.md R-TEST-1, R-TEST-2
// robustness).
#[cfg(test)]
mod tests;
