//! The rows of a table access, with their ids: the object rows of a
//! searched UPDATE or DELETE [X3.135-1986 §8.12 GR1, §8.5 GR1], and the
//! rows an updatable cursor may cover. Split from exec.rs (RULES.md
//! R-SIZE-2).

use std::sync::Arc;

use super::held::HeldRows;
use super::{Found, Runner, broken, eval_cond, fetch, probe_index};
use crate::db::State;
use crate::error::Result;
use crate::logical::{Truth, pushed_conjuncts};
use crate::physical::Plan;
use crate::storage::{Condition, Row, RowId};
use crate::txn::{TxId, checked};

/// Rows with their ids, each read when it is pulled.
pub type Entries = Box<dyn Iterator<Item = Result<(RowId, Row)>> + Send>;

/// The rows, with their ids, produced by a table access: a scan or index
/// scan, possibly filtered, possibly under a projection (which is ignored),
/// each read when it is pulled, and only while the statement is not
/// cancelled (RULES.md D44). UPDATE, DELETE and updatable cursors need the
/// ids to change the rows.
///
/// # Errors
///
/// `plan` is not a table access or names a table or index that does not
/// exist (SQLCODE -910); reading the rows, as they are pulled, as
/// `row_entries` says.
pub fn table_entries(plan: Plan, db: &Arc<State>, tx: TxId) -> Result<Entries> {
    match plan {
        Plan::Project { input, .. } => table_entries(*input, db, tx),
        Plan::SeqScan { table, columns, .. } => {
            Ok(Box::new(checked(Arc::clone(db.storage_of(&table)?).scan(tx, &columns), db.tx.stop_of(tx))))
        }
        Plan::IndexScan { table, index, probe, reverse, .. } => {
            let Found { storage, columns, entries } = probe_index(db, &table, &index, &probe, reverse)?;
            storage.read_by_condition(tx, &Condition { columns: columns.clone(), probe })?;
            let rows = entries.into_iter().filter_map(move |(key, id)| fetch(&*storage, &columns, tx, &key, id).transpose());
            Ok(Box::new(checked(rows, db.tx.stop_of(tx))))
        }
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
        other @ (Plan::Product { .. }
        | Plan::HashJoin { .. }
        | Plan::OuterJoin { .. }
        | Plan::Aggregate { .. }
        | Plan::Distinct { .. }
        | Plan::Append { .. }
        | Plan::SetOp { .. }
        | Plan::Values { .. }
        | Plan::System { .. }
        | Plan::Sort { .. }) => Err(broken(&format!("not a table access: {other:?}"))),
    }
}

/// The rows `table_entries` gives, every one read now, before any is
/// changed: the object rows of a searched UPDATE or DELETE [X3.135-1986
/// §8.12 GR1, §8.5 GR1]. Held up to WORK_MEMORY, the rest spilled
/// (`exec/held.rs`, RULES.md D38).
///
/// # Errors
///
/// A row's search condition fails, the statement is cancelled or runs past
/// STATEMENT_TIMEOUT (SQLCODE -920), `plan` is not a table access or names
/// a table or index that does not exist (SQLCODE -910), or the rows cannot
/// be spilled (SQLCODE -918 if the area cannot be reserved).
pub fn row_entries(plan: Plan, db: &Arc<State>, tx: TxId) -> Result<HeldRows<(RowId, Row)>> {
    let mut held = HeldRows::new(db.list_space());
    for entry in table_entries(plan, db, tx)? {
        held.push(entry?)?;
    }
    Ok(held)
}
