//! The rows of a table access, with their ids: the object rows of a
//! searched UPDATE or DELETE [X3.135-1986 §8.12 GR1, §8.5 GR1], and the
//! rows an updatable cursor may cover.

use crate::exec::{Db, Opening};
use std::sync::Arc;

use super::held::HeldRows;
use super::scan::{Found, probe_index};
use super::{Runner, broken, eval_cond};
use crate::catalog::{KeyShape, QualifiedName};
use crate::error::Result;
use crate::index::{Key, KeyProjection, Probe};
use crate::logical::{Truth, pushed_conjuncts};
use crate::physical::{Decided, Plan};
use crate::storage::{Near, Row, RowId, TableStorage};
use crate::txn::{TxId, checked};
use crate::value::Value;

/// What the fetches of one scan of an index share: the columns they read,
/// chosen once, and what the table keeps from read to read.
pub(in crate::exec) struct Reading {
    columns: Option<Vec<usize>>,
    near: Near,
    projection: Option<Arc<dyn KeyProjection>>,
}

impl Reading {
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
        Reading { columns, near: Near::default(), projection }
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
    let Reading { columns, near, projection } = reading;
    let Some(row) = storage.get_near(tx, id, columns.as_deref(), near)? else { return Ok(None) };
    let Some(mut seen) = ix.key_of(&row) else { return Ok(None) };
    if let Some(project) = projection {
        seen = project.project(&seen)?;
    }
    Ok((seen == *key).then_some((id, row)))
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
/// ids to change the rows.
///
/// # Errors
///
/// `plan` is not a table access or names a table or index that does not
/// exist (SQLCODE -910); reading the rows, as they are pulled, as
/// `row_entries` says.
pub fn table_entries(plan: Plan, db: &(impl Opening + ?Sized), tx: TxId) -> Result<Entries> {
    entries_in(plan, &db.opening(), tx)
}

/// `table_entries`' work, in `db`.
fn entries_in(plan: Plan, db: &Db, tx: TxId) -> Result<Entries> {
    match plan {
        Plan::Project { input, .. } => entries_in(*input, db, tx),
        Plan::SeqScan { table, columns, .. } => {
            Ok(Box::new(checked(Arc::clone(db.storage_of(&table)?).scan(tx, &columns), db.tx.stop_of(tx))))
        }
        Plan::IndexScan { table, index, probe, reverse, decided, .. } => index_entries((&table, &index, probe, reverse), decided, db, tx),
        Plan::Filter { cond, input, .. } => {
            let rows: Entries = match (*input, pushed_conjuncts(&cond)) {
                (Plan::SeqScan { table, columns, .. }, Some(condition)) => {
                    Box::new(checked(Arc::clone(db.storage_of(&table)?).scan_where(tx, &columns, &condition), db.tx.stop_of(tx)))
                }
                (input, _) => table_entries(input, db, tx)?,
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
        other @ (Plan::Product { .. }
        | Plan::Nearest(_)
        | Plan::HashJoin { .. }
        | Plan::OuterJoin { .. }
        | Plan::IndexJoin { .. }
        | Plan::MergeJoin(_)
        | Plan::SemiJoin(_)
        | Plan::RangeJoin(_)
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
        | Plan::Counted(_)) => Err(broken(&format!("not a table access: {other:?}"))),
    }
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
    let judge = move |(id, row): (RowId, Row)| -> Result<Option<(RowId, Row)>> {
        let Some(decided) = &decided else { return Ok(Some((id, row))) };
        Ok((eval_cond(&decided.cond, &row, &runner)? == Truth::True).then_some((id, row)))
    };
    let rows = checked(entries, db.tx.stop_of(tx)).filter_map(move |entry| {
        let fetched = entry.and_then(|(key, id)| fetch(&*storage, &ix, &mut reading, (tx, id), &key));
        fetched.and_then(|found| found.map_or(Ok(None), &judge)).transpose()
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
/// STATEMENT.TIMEOUT_MS (SQLCODE -920), `plan` is not a table access or names
/// a table or index that does not exist (SQLCODE -910), or the rows cannot
/// be spilled (SQLCODE -918 if the area cannot be reserved).
pub fn row_entries(plan: Plan, db: &(impl Opening + ?Sized), tx: TxId) -> Result<HeldRows<(RowId, Row)>> {
    let db = db.opening();
    let mut held = HeldRows::new(db.list_space());
    for entry in entries_in(plan, &db, tx)? {
        held.push(entry?)?;
    }
    Ok(held)
}

#[cfg(test)]
mod tests;
