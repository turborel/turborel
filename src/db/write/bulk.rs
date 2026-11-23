//! \[EXTENSION\] Rows inserted together (RULES.md D530): the rows of an
//! INSERT, of a query or of many rows of values, and of a bulk copy, put in
//! their table a batch at a time. The table's lock is taken once; each
//! batch's rows go to its storage, then to the log as one record
//! (`log/record/batch.rs`), then to each index, in the rows' order, so each
//! index is as inserting them one at a time leaves it: its entries, its
//! pages and the estimates read from them alike.
//!
//! Each row is undone as `insert_row`'s is, by its own undo entry and those
//! of its index entries, and redone from the record as an insert of its
//! own, so what a statement does, and what a crash keeps of it, is as if
//! its rows were inserted one at a time. A table whose rows that would not
//! be so for, a partition or a partitioned table, one whose rows are
//! sealed, or a row holding a large object, takes them one at a time.
//!
//! A batch's rows are read where the statement holds them, never copied
//! for it (RULES.md D531): its storage encodes each as it is held, its log
//! record is made from them, and each index's keys, made once, are both
//! inserted and kept for undo, one undo entry an index a batch. What every
//! batch needs, the table's storage, its indexes and its checks, is found
//! once a statement.

use std::sync::Arc;

use super::super::integrity::{Effects, RowsChecked};
use crate::catalog::{IndexDesc, QualifiedName};
use crate::db::State;
use crate::error::{Code, Result, SqlError};
use crate::exec::HeldRows;
use crate::index::{Index, Key};
use crate::log::record::insert_rows_record;
use crate::params::Load;
use crate::spill::row_bytes;
use crate::storage::{Row, RowId, TableStorage};
use crate::txn::{Txn, Undo, UndoEntry};
use crate::value::Value;

/// \[DESIGN\] The rows a batch holds at most: enough that the costs of a
/// batch are shared by many rows, few enough that its keys are held in
/// memory once at most.
const BATCH_ROWS: usize = 4096;
/// \[DESIGN\] The bytes of rows one record holds, about, at most: so a
/// record stays well within a log segment and a replay holds little.
const RECORD_BYTES: usize = 1 << 20;

pub(super) mod load;

/// An index the rows of a table go to, and where its entries are.
struct Indexed {
    desc: IndexDesc,
    partition: Option<u64>,
    index: Arc<dyn Index>,
    /// Whether a load holds its entries back, to build it whole as it ends
    /// (RULES.md D532).
    held: bool,
}

/// What each batch of one statement's rows of a table needs, found once.
pub(in crate::db) struct Together<'s> {
    table: &'s QualifiedName,
    storage: Arc<dyn TableStorage>,
    /// Whether each row inserted takes an undo entry of its own: its
    /// engine cannot undo its own changes.
    undone: bool,
    /// Whether its changes are logged.
    logs: bool,
    indexes: Vec<Indexed>,
    /// The rows of a group, if a load writes its rows straight as frozen
    /// groups (RULES.md D533).
    grouped: Option<usize>,
}

/// The rows of a batch gone in: their ids, in order, and of each whether
/// its key was its key's only entry in every unique index.
pub(in crate::db) struct Gone {
    ids: Vec<RowId>,
    alone: Vec<bool>,
}

impl State {
    /// Whether rows of `table` are inserted together: it is no partition
    /// and not partitioned, and its rows are not sealed.
    ///
    /// # Errors
    ///
    /// As `seal_of`.
    pub(crate) fn inserts_together(&self, table: &QualifiedName) -> Result<bool> {
        let p = self.catalog.partitioning();
        let whole = p.is_empty() || (!p.is_partitioned(table) && p.ancestors(table).is_empty());
        Ok(whole && self.seal_of(table)?.is_none())
    }

    /// What the batches of `table` need, of a statement: its storage, how
    /// it logs and undoes, and its indexes, each found once.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if one of its indexes does not exist; as
    /// `storage_of` and `capabilities`.
    pub(in crate::db) fn together<'s>(&self, table: &'s QualifiedName) -> Result<Together<'s>> {
        let caps = self.capabilities(table)?;
        let mut indexes = Vec::new();
        for (desc, partition) in self.table_indexes(table) {
            let index = self.indexes.get(&desc.name).map(Arc::clone);
            let index = index.ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] the index {} does not exist", desc.name)))?;
            indexes.push(Indexed { desc: desc.into_owned(), partition, index, held: false });
        }
        let storage = Arc::clone(self.storage_of(table)?);
        let logs = Self::logs_changes(caps.persistence);
        Ok(Together { table, storage, undone: caps.undo == Undo::Log, logs, indexes, grouped: None })
    }

    /// [X3.135-1986 §8.7] Inserts `rows` into the table of `together`, with
    /// their index entries, as `insert_row` inserts each: their row ids, in
    /// order, and whether each was alone in every unique index, noted in
    /// `tx` too if `notes`. A row inserted by `insert_row` is taken not to
    /// have been, as it notes it itself.
    ///
    /// # Errors
    ///
    /// As `insert_row`'s.
    pub(in crate::db) fn insert_rows(&self, tx: &Txn, together: &Together<'_>, rows: &[Row], notes: bool) -> Result<Gone> {
        let table = together.table;
        for row in rows {
            crate::geometry::registered::check_row(&*self.catalog, table, row)?;
        }
        let one_at_a_time = rows.len() < 2 || rows.iter().flatten().any(Value::needs_keeping) || !self.inserts_together(table)?;
        if one_at_a_time {
            let ids = rows.iter().map(|row| self.insert_row(tx, table, row)).collect::<Result<Vec<_>>>()?;
            return Ok(Gone { alone: vec![false; ids.len()], ids });
        }
        self.prepare_write(tx, table)?;
        // After any wait for a lock: a checkpoint never waits on a waiter.
        let _gate = self.tx.gate.enter();
        let ids = self.stored_together(tx, together, rows)?;
        let mut alone = vec![true; ids.len()];
        for ix in &together.indexes {
            index_rows(tx, ix, (rows, &ids), (notes, &mut alone))?;
        }
        Ok(Gone { ids, alone })
    }

    /// Puts `rows` in the storage of `together`, as a frozen group if a
    /// load writes them so (RULES.md D533), else each with its undo entry
    /// if it takes one, and logs them a record at a time: their ids.
    fn stored_together(&self, tx: &Txn, together: &Together<'_>, rows: &[Row]) -> Result<Vec<RowId>> {
        let table = together.table;
        let loading = together.grouped.and_then(|_| together.storage.loading());
        let grouped = loading.map(|l| l.insert_group(tx.id, rows)).transpose()?.flatten();
        let given = grouped.is_some();
        let mut ids = grouped.unwrap_or_else(|| Vec::with_capacity(rows.len()));
        let mut from = 0;
        while let Some(rest) = rows.get(from..).filter(|r| !r.is_empty()) {
            let taken = record_rows(rest);
            let part = rest.get(..taken).unwrap_or_default();
            for row in part.iter().filter(|_| !given) {
                let id = together.storage.insert_from(tx.id, row)?;
                ids.push(id);
                if together.undone {
                    tx.log(UndoEntry::Inserted { table: table.clone(), storage: Arc::clone(&together.storage), id })?;
                }
            }
            if together.logs {
                let made = ids.get(from..from.saturating_add(taken)).unwrap_or_default();
                let record = insert_rows_record(tx.id, (table, self.generation_of(table)), made, part);
                tx.log_encoded(&record, u64::try_from(made.len()).unwrap_or(u64::MAX))?;
            }
            from = from.saturating_add(taken);
        }
        Ok(ids)
    }
}

/// Adds the entries of `rows`, of row ids `ids`, to the index of `ix`, in
/// their order, as `index_add` adds each, and their undo entry: of a
/// unique index, each row not alone under its key marked so in `alone`,
/// and each noted in `tx` if `notes`.
fn index_rows(tx: &Txn, ix: &Indexed, (rows, ids): (&[Row], &[RowId]), (notes, alone): (bool, &mut [bool])) -> Result<()> {
    let mut entries: Vec<(Key, RowId)> = Vec::with_capacity(rows.len());
    let mut each = Vec::new();
    for ((row, &id), lone) in rows.iter().zip(ids).zip(alone.iter_mut()) {
        let key = ix.desc.computed_key(row)?;
        if !ix.desc.holds(&key) {
            continue;
        }
        let id = State::entry_id(ix.partition, id)?;
        // Of a unique index, whether the key's only entry, so its
        // constraint need not look for another row; none of an index a
        // load holds back, which is no unique one.
        let only = if ix.held { false } else { ix.index.insert_from(&key, id, ix.desc.unique)? };
        if ix.desc.unique {
            *lone = *lone && only;
            each.push((id, only));
        }
        entries.push((key, id));
    }
    if notes && !each.is_empty() {
        tx.note_alone_rows(&ix.desc.name, &mut each.into_iter())?;
    }
    if entries.is_empty() {
        return Ok(());
    }
    tx.log(UndoEntry::IndexInsertedRows { table: ix.desc.table.clone(), index: Arc::clone(&ix.index), entries })
}

/// How many of `rows`, at least one, one record holds: as many as come to
/// `RECORD_BYTES`.
fn record_rows(rows: &[Row]) -> usize {
    let mut bytes = 0_usize;
    let fit = rows.iter().take_while(|row| {
        bytes = bytes.saturating_add(row_bytes(row));
        bytes <= RECORD_BYTES
    });
    fit.count().max(1)
}

/// [X3.135-1986 §8.7 GR1c] Inserts the rows `rows` holds into `table`, of
/// transaction `tx`, noting each in `effects`: a batch at a time, read
/// where they are held, if the table takes its rows together; else one at
/// a time, each where its partition holds it (RULES.md D155). The insert
/// is part `load` of a load, if given; else, of a batch of rows or more, a
/// load of its own (RULES.md D532); a load's part that ends it ends it.
///
/// # Errors
///
/// As `State::insert_rows`, `State::insert_routed`, `Effects::inserted`
/// and `load::end`.
pub(super) fn insert_held(
    (st, tx): (&State, &Txn),
    table: &QualifiedName,
    rows: &mut HeldRows<Row>,
    effects: &mut Effects<'_>,
    load: Option<Load>,
) -> Result<()> {
    let load = load.or_else(|| (rows.len() >= BATCH_ROWS).then_some(Load::Alone));
    if rows.len() < 2 || !st.inserts_together(table)? {
        rows.visit(&mut |row| st.insert_routed(tx, table, row).and_then(|(leaf, id)| effects.inserted(&leaf, id, row)))?;
        return load::end(st, tx, load);
    }
    // Checked as each goes in where it may be, so most need no note.
    let mut checked = st.rows_checked(table, effects);
    let mut together = st.together(table)?;
    if load.is_some() && checked.is_some() {
        load::hold_back(st, tx, &mut together)?;
    }
    let each = together.grouped.unwrap_or(BATCH_ROWS).max(1);
    rows.visit_slices(&mut |held| {
        for batch in held.chunks(each) {
            inserted(st, (tx, &together), batch, (&mut checked, &mut *effects))?;
        }
        Ok(())
    })?;
    load::end(st, tx, load)
}

/// Inserts `batch` into the table of `together` and notes each row that
/// needs it in `effects`.
fn inserted(
    st: &State,
    (tx, together): (&Txn, &Together<'_>),
    batch: &[Row],
    (checked, effects): (&mut Option<RowsChecked<'_>>, &mut Effects<'_>),
) -> Result<()> {
    if let Some(c) = checked.as_mut() {
        for row in batch {
            c.check((st, tx), row)?;
        }
    }
    let gone = st.insert_rows(tx, together, batch, checked.is_none())?;
    for ((row, id), alone) in batch.iter().zip(gone.ids).zip(gone.alone) {
        if checked.as_ref().is_none_or(|c| c.needs_note(alone)) {
            effects.inserted(together.table, id, row)?;
        }
    }
    Ok(())
}
