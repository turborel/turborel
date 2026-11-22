//! The rows of a table access, with their ids: the object rows of a
//! searched UPDATE or DELETE [X3.135-1986 §8.12 GR1, §8.5 GR1], and the
//! rows an updatable cursor may cover.

use crate::exec::{Db, Opening};
use std::sync::Arc;

use super::held::HeldRows;
use super::scan::{Found, probe_index};
use super::{Runner, broken, eval_cond};
use crate::catalog::{KeyShape, QualifiedName};
use crate::engine::key::{decode_key, encode_columns};
use crate::error::Result;
use crate::index::{Key, KeyProjection, Probe};
use crate::logical::{Truth, pushed_conjuncts};
use crate::noted::{self, Noted, Notes};
use crate::physical::{Decided, Plan};
use crate::storage::{Conjunction, Near, Row, RowId, TableStorage, row_id_of};
use crate::txn::{TxId, checked, counted};
use crate::value::{DataType, Value};

/// What the fetches of one scan of an index share: the columns they read,
/// chosen once, and what the table keeps from read to read.
pub(in crate::exec) struct Reading {
    columns: Option<Vec<usize>>,
    near: Near,
    projection: Option<Arc<dyn KeyProjection>>,
    /// \[EXTENSION\] The notes of the operator it reads for, if it is
    /// counted, and the entries, fetches and rechecks it adds to them as it
    /// is dropped (RULES.md D621).
    notes: Option<Arc<Notes>>,
    counts: [usize; 3],
    /// The key of the row last fetched, as an index holds it, made in
    /// place of each row's (`fetch_encoded`).
    key: Vec<u8>,
}

impl Drop for Reading {
    fn drop(&mut self) {
        let [entries, fetched, rechecked] = self.counts;
        let notes = self.notes.as_deref();
        noted::add(notes, Noted::Entries, entries);
        noted::add(notes, Noted::Fetched, fetched);
        noted::add(notes, Noted::Rechecked, rechecked);
    }
}

impl Reading {
    /// Counts an entry read without its row.
    pub(in crate::exec) fn entry(&mut self) {
        self.count(0);
    }

    /// Counts a row's condition judged again.
    pub(in crate::exec) fn rechecked(&mut self) {
        self.count(2);
    }

    /// Counts one more of its count `at`.
    fn count(&mut self, at: usize) {
        if let Some(n) = self.counts.get_mut(at) {
            *n = n.saturating_add(1);
        }
    }

    /// Independent projected probes can change from sparse to clustered
    /// row ids. Renew the leaf trial without dropping the transaction's
    /// cached bookkeeping or the row-byte allocation.
    pub(in crate::exec) fn next_probe(&mut self) {
        if self.projection.is_some() {
            self.near.misses = 0;
        }
    }

    /// For the scan of index `ix` by a statement that reads only `needed`
    /// and the columns the index's key reads, if it names them; else every
    /// column.
    pub(in crate::exec) fn of(ix: &KeyShape, needed: Option<&[usize]>, projection: Option<Arc<dyn KeyProjection>>) -> Reading {
        let columns = needed.map(|needed| {
            let mut read: Vec<usize> = needed.iter().chain(&ix.reads).copied().collect();
            read.sort_unstable();
            read.dedup();
            read
        });
        Reading { columns, near: Near::default(), projection, notes: noted::noting(), counts: [0; 3], key: Vec::new() }
    }
}

/// Row `id` of `storage`, as `tx` sees it, if that version has `key` in the
/// index `ix`: an index holds the entries of versions `tx` may not
/// see (`Index`). Of a statement that reads only some columns, only those
/// are read (`TableStorage::get_columns`).
pub(in crate::exec) fn fetch(
    storage: &dyn TableStorage,
    ix: &KeyShape,
    reading: &mut Reading,
    (tx, id): (TxId, RowId),
    key: &Key,
) -> Result<Option<(RowId, Row)>> {
    reading.entry();
    let Reading { columns, near, projection, counts, .. } = reading;
    let Some(row) = storage.get_near(tx, id, columns.as_deref(), near)? else { return Ok(None) };
    fetched(counts);
    // \[DESIGN\] (RULES.md D501) A key of the row's own values is checked
    // in place, with no key made of the row.
    if projection.is_none() && !ix.is_collated() && !ix.has_expressions() {
        return Ok(key.is_of(&row, &ix.columns).then_some((id, row)));
    }
    let Some(mut seen) = ix.key_of(&row) else { return Ok(None) };
    if let Some(project) = projection {
        seen = project.project(&seen)?;
    }
    Ok((seen == *key).then_some((id, row)))
}

/// Counts a row fetched, of `counts` (`Reading`).
fn fetched(counts: &mut [usize; 3]) {
    counts[1] = counts[1].saturating_add(1);
}

/// \[DESIGN\] (RULES.md D821) As `fetch`, of an entry whose key is the
/// bytes `key` of columns of `types`, of an index `fetch` checks in place:
/// the row's key columns are encoded as the index made its entry of them,
/// and the two compared as bytes. Equal bytes are of equal values; bytes
/// not equal are decoded and compared as `fetch` compares, so it keeps
/// every row `fetch` keeps, and no other.
///
/// # Errors
///
/// As `TableStorage::get_near`; as `encode_key` for a row whose key no
/// index could hold; SQLCODE -916 for an entry whose key is malformed.
pub(in crate::exec) fn fetch_encoded(
    storage: &dyn TableStorage,
    ix: &KeyShape,
    reading: &mut Reading,
    (tx, id): (TxId, RowId),
    (key, types): (&[u8], &[DataType]),
) -> Result<Option<Row>> {
    reading.entry();
    let Reading { columns, near, counts, key: made, .. } = reading;
    let Some(row) = storage.get_near(tx, id, columns.as_deref(), near)? else { return Ok(None) };
    fetched(counts);
    made.clear();
    encode_columns(&row, &ix.columns, types, made)?;
    if made.as_slice() == key {
        return Ok(Some(row));
    }
    Ok(Key(decode_key(key, types)?).is_of(&row, &ix.columns).then_some(row))
}

/// \[DESIGN\] (RULES.md D34) The row of an entry of row `id` under `key`
/// in an index on `columns`, as `tx` sees it, if it does: of a scan
/// covering the columns read, the row `width` columns wide, made of the
/// key at `columns` and nulls elsewhere, if every reader sees the row's
/// one version (`TableStorage::visible_to_all`); else read (`fetch`), of
/// the columns the statement uses.
pub(super) fn entry_row(
    storage: &dyn TableStorage,
    (ix, width): (&KeyShape, Option<usize>),
    reading: &mut Reading,
    (tx, id): (TxId, RowId),
    key: Key,
) -> Result<Option<Row>> {
    if let Some(width) = width.filter(|_| reading.projection.is_none())
        && storage.visible_to_all(id)
    {
        reading.entry();
        let mut row = vec![Value::Null; width];
        for (&column, value) in ix.columns.iter().zip(key.0) {
            if let Some(slot) = row.get_mut(column) {
                *slot = value;
            }
        }
        return Ok(Some(row));
    }
    Ok(fetch(storage, ix, reading, (tx, id), &key)?.map(|(_, row)| row))
}

/// Rows with their ids, each read when it is pulled.
pub type Entries = Box<dyn Iterator<Item = Result<(RowId, Row)>> + Send>;

/// The rows, with their ids, produced by a table access: a scan, an index
/// scan or the index scans of an OR's ranges appended, possibly filtered, possibly under a projection (which is ignored),
/// each read when it is pulled, and only while the statement is not
/// cancelled (RULES.md D44). UPDATE, DELETE and updatable cursors need the
/// ids to change the rows. Of any other plan, its object table's scan
/// located, each row's id is its last column (RULES.md D775).
///
/// # Errors
///
/// `plan` names a table or index that does not exist (SQLCODE -910);
/// reading the rows, as they are pulled, as `row_entries` says, and a row
/// of a plan not a table access with no id (SQLCODE -910).
pub fn table_entries(plan: Plan, db: &(impl Opening + ?Sized), tx: TxId) -> Result<Entries> {
    object_entries(plan, &db.opening(), tx)
}

/// `table_entries`' work, in `db`: of a table access, its entries; of any
/// other plan, its rows, each id taken off its row (RULES.md D775).
fn object_entries(plan: Plan, db: &Db, tx: TxId) -> Result<Entries> {
    if !is_table_access(&plan) {
        return Ok(located_entries(plan, db, tx));
    }
    entries_in(plan, db, tx)
}

/// The entries of `plan`, a table access, in `db`.
fn entries_in(plan: Plan, db: &Db, tx: TxId) -> Result<Entries> {
    match plan {
        Plan::Project { input, .. } => entries_in(*input, db, tx),
        Plan::SeqScan { table, columns, .. } => {
            Ok(Box::new(counted(Arc::clone(db.storage_of(&table)?).scan(tx, &columns), db.tx.stop_of(tx), |_| 1)))
        }
        Plan::IndexScan { table, index, probe, slots, reverse, decided, .. } => {
            let probe = super::scan::slotted(probe, &slots, db)?;
            index_entries((&table, &index, probe, reverse), decided, db, tx)
        }
        Plan::Filter { cond, input, .. } => {
            let rows = match pushed_conjuncts(&cond) {
                Some(condition) => told_entries(*input, &condition, db, tx)?,
                None => table_entries(*input, db, tx)?,
            };
            let runner = Runner::new(db, tx);
            Ok(Box::new(rows.filter_map(move |read| match read.and_then(|(id, row)| Ok((eval_cond(&cond, &row, &runner)?, id, row))) {
                Ok((Truth::True, id, row)) => Some(Ok((id, row))),
                Ok((Truth::False | Truth::Unknown, ..)) => None,
                Err(e) => Some(Err(e)),
            })))
        }
        // The index scans of an OR's ranges, none overlapping
        // (`physical/access.rs`): each row once.
        Plan::Append { left, right, .. } => Ok(Box::new(table_entries(*left, db, tx)?.chain(table_entries(*right, db, tx)?))),
        // [EXTENSION] of a data change EXPLAIN ANALYZE runs (RULES.md D622).
        Plan::Counted(c) => super::counted::entries_through(*c, db, |input| table_entries(input, db, tx)),
        other @ (Plan::Product { .. }
        | Plan::Nearest(_)
        | Plan::HashJoin { .. }
        | Plan::OuterJoin { .. }
        | Plan::IndexJoin { .. }
        | Plan::MergeJoin(_)
        | Plan::RangeJoin(_)
        | Plan::SemiJoin(_)
        | Plan::MarkJoin(_)
        | Plan::Aggregate { .. }
        | Plan::Distinct { .. }
        | Plan::SetOp { .. }
        | Plan::Values { .. }
        | Plan::System { .. }
        | Plan::Sort { .. }
        | Plan::MergeAppend { .. }
        | Plan::Limit { .. }
        | Plan::Recursive { .. }
        | Plan::Unnest(..)
        | Plan::Window { .. }
        | Plan::Polymorphic(..)
        | Plan::Recognize { .. }
        | Plan::WorkTable { .. }
        | Plan::Transition { .. }
        | Plan::Parallel(..)
        | Plan::Transfer(_)) => Err(broken(&format!("not a table access: {other:?}"))),
    }
}

/// Whether `plan` is a table access whose entries `table_entries` reads
/// with their ids: a scan, an index scan or an OR's index scans appended,
/// filtered or projected, counted or not. Any other plan of a table's rows
/// gives each row's id after its columns (RULES.md D775).
pub fn is_table_access(plan: &Plan) -> bool {
    match plan {
        Plan::SeqScan { .. } | Plan::IndexScan { .. } => true,
        Plan::Project { input, .. } | Plan::Filter { input, .. } => is_table_access(input),
        Plan::Append { left, right, .. } => is_table_access(left) && is_table_access(right),
        Plan::Counted(c) => is_table_access(&c.input),
        Plan::Product { .. }
        | Plan::Nearest(_)
        | Plan::HashJoin { .. }
        | Plan::OuterJoin { .. }
        | Plan::IndexJoin { .. }
        | Plan::MergeJoin(_)
        | Plan::RangeJoin(_)
        | Plan::SemiJoin(_)
        | Plan::MarkJoin(_)
        | Plan::Aggregate { .. }
        | Plan::Distinct { .. }
        | Plan::SetOp { .. }
        | Plan::Values { .. }
        | Plan::System { .. }
        | Plan::Sort { .. }
        | Plan::MergeAppend { .. }
        | Plan::Limit { .. }
        | Plan::Recursive { .. }
        | Plan::Unnest(..)
        | Plan::Window { .. }
        | Plan::Polymorphic(..)
        | Plan::Recognize { .. }
        | Plan::WorkTable { .. }
        | Plan::Transition { .. }
        | Plan::Parallel(..)
        | Plan::Transfer(_) => false,
    }
}

/// \[DESIGN\] The rows of `plan`, whose table's scan is located, with
/// their ids, each row's id its last column, taken off it (RULES.md D775).
/// Each row pulled fails as the plan's rows do, or with SQLCODE -910 if
/// it has no row id.
fn located_entries(plan: Plan, db: &Db, tx: TxId) -> Entries {
    Box::new(super::opened(plan, db, tx).map(|read| {
        let mut row = read?;
        let id = row.pop().as_ref().map(row_id_of).transpose()?.flatten();
        Ok((id.ok_or_else(|| broken("[DESIGN] an object row has no row id"))?, row))
    }))
}

/// The rows with their ids of `input`, a filter's input, the scan of a
/// table told the simple conjuncts `condition` of the filter, through a
/// counted scan too (RULES.md D378).
///
/// # Errors
///
/// As `table_entries`.
fn told_entries(input: Plan, condition: &Conjunction, db: &Db, tx: TxId) -> Result<Entries> {
    if let Plan::Counted(c) = input {
        return super::counted::entries_through(*c, db, |scan| told_entries(scan, condition, db, tx));
    }
    let Plan::SeqScan { table, columns, .. } = &input else { return table_entries(input, db, tx) };
    Ok(Box::new(counted(Arc::clone(db.storage_of(table)?).scan_where(tx, columns, condition), db.tx.stop_of(tx), |_| 1)))
}

/// The rows, with their ids, the probe `probe` of index `index` of `table`
/// finds, read `reverse` if so; of an XML query the index answers, those
/// it answers (`exec/answered.rs`). The conjunct the scan decides of each
/// entry, `decided` (`Plan::IndexScan`), is judged of each row read, as
/// its filter would.
///
/// # Errors
///
/// As `table_entries`.
fn index_entries(
    (table, index, probe, reverse): (&QualifiedName, &str, Probe, bool),
    decided: Option<Box<Decided>>,
    db: &Db,
    tx: TxId,
) -> Result<Entries> {
    if let Probe::Xml(asked) = &probe
        && asked.answered
    {
        return super::answered::answered_entries(db, tx, (table, index, asked), None);
    }
    let Found { storage, ix, entries, projection, read } = probe_index(db, table, index, &probe, (reverse, true))?;
    read.record(&*storage, tx)?;
    let mut reading = Reading::of(&ix, None, projection);
    let runner = Runner::new(db, tx);
    let judge = move |(id, row): (RowId, Row), judged: &mut Reading| -> Result<Option<(RowId, Row)>> {
        let Some(decided) = &decided else { return Ok(Some((id, row))) };
        judged.rechecked();
        Ok((eval_cond(&decided.cond, &row, &runner)? == Truth::True).then_some((id, row)))
    };
    let rows = counted(entries, db.tx.stop_of(tx), |_| 1).filter_map(move |entry| {
        let fetched = entry.and_then(|(key, id)| fetch(&*storage, &ix, &mut reading, (tx, id), &key));
        fetched.and_then(|found| found.map_or(Ok(None), |row| judge(row, &mut reading))).transpose()
    });
    Ok(Box::new(checked(rows, db.tx.stop_of(tx))))
}

/// The rows `table_entries` gives, every one read now, before any is
/// changed: the object rows of a searched UPDATE or DELETE [X3.135-1986
/// §8.12 GR1, §8.5 GR1]. Held up to MEMORY.WORK_BYTES, the rest spilled
/// (`exec/held.rs`, RULES.md D38).
///
/// # Errors
///
/// A row's search condition fails, the statement is cancelled or runs past
/// STATEMENT.TIMEOUT_MS (SQLCODE -920), `plan` names a table or index that
/// does not exist or a row of a plan not a table access has no id (SQLCODE
/// -910), or the rows cannot be spilled (SQLCODE -918 if the area cannot
/// be reserved).
pub fn row_entries(plan: Plan, db: &(impl Opening + ?Sized), tx: TxId) -> Result<HeldRows<(RowId, Row)>> {
    let db = db.opening();
    let mut held = HeldRows::new(db.list_space());
    for entry in object_entries(plan, &db, tx)? {
        held.push(entry?)?;
    }
    Ok(held)
}

#[cfg(test)]
mod tests;
