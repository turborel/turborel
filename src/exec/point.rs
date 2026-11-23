//! \[DESIGN\] Direct point operations (RULES.md D752). A kept plan that is
//! one probe of a whole unique key, its columns given as the table holds
//! them, finds one row at most, so it is run without the executor's
//! operators. Its probe is placed with the run's values and the index
//! read, and the row of the one entry it sees is read of the columns
//! asked for alone (D751). It reads, records its read and counts its entries as the
//! index scan does (`scan::indexed`), so its rows, errors and conflicts
//! are those of the plan. Any other plan is run by the executor.

use std::mem;

mod keyed;

use super::entries::{Reading, entry_row, fetch};
use super::scan::{Found, probe_index, read_row};
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::error::Result;
use crate::index::Probe;
use crate::logical::Expr;
use crate::physical::{Plan, ProbeSlot, keepable, placed};
use crate::storage::{Row, RowId};
use crate::txn::{TxId, counted};
use crate::value::Value;

/// Whether `st` runs a kept point plan as a point operation
/// (EXECUTOR.POINT_OPERATIONS, RULES.md D752).
pub(crate) fn point_operations(st: &State) -> bool {
    if let Value::Char(s) = st.settings.get(crate::settings::EXECUTOR_POINT_OPERATIONS) { s != crate::settings::OFF } else { true }
}

/// \[DESIGN\] A kept plan run as one point read (RULES.md D752). It holds
/// the probe of a whole unique key of `index` and the columns of its row
/// given.
#[derive(Debug, Clone)]
pub(crate) struct PointRead {
    table: QualifiedName,
    index: String,
    probe: Probe,
    slots: Vec<ProbeSlot>,
    /// The table's columns read of the row found.
    needed: Vec<usize>,
    /// Whether the index holds every column read (`Plan::IndexScan`).
    covering: bool,
    /// Whether the row's location follows its columns.
    located: bool,
    /// The columns given, by their places in the row read; none if the
    /// row is given as read.
    outputs: Option<Vec<usize>>,
    /// Whether no column is given twice, so each is moved out of the row.
    distinct: bool,
    /// What probes its index by a whole key, if it is read so (`keyed.rs`).
    keyed: Option<keyed::Keyed>,
}

impl PointRead {
    /// The point read `plan` is, if it is one. It is an index scan of a
    /// whole unique key (`physical::keepable`), alone or under a
    /// projection of its columns. The index is read from `st`, the state it was planned
    /// in.
    pub(crate) fn of(plan: &Plan, st: &State) -> Option<PointRead> {
        if !keepable(plan, st) {
            return None;
        }
        let (outputs, scan) = scan_of(plan)?;
        let Plan::IndexScan { table, index, probe, slots, covering, columns, located, decided, .. } = scan else { return None };
        // A scan deciding a condition of its own judges each row it reads
        // (`scan.rs`), which a point read does not.
        if decided.is_some() {
            return None;
        }
        // An XML lookup answered by its index is no point read.
        if let Probe::Xml(_) = probe {
            return None;
        }
        let distinct = outputs.as_ref().is_none_or(|o| o.iter().enumerate().all(|(i, c)| !o.get(..i).unwrap_or_default().contains(c)));
        let keyed = keyed::Keyed::of(st, (table, index), probe, (columns, *covering));
        Some(PointRead {
            keyed,
            table: table.clone(),
            index: index.clone(),
            probe: probe.clone(),
            slots: slots.clone(),
            needed: columns.clone(),
            covering: *covering,
            located: *located,
            outputs,
            distinct,
        })
    }

    /// The row `tx` sees under the probe placed with `values`, its slots'
    /// values, of the columns given; none if it sees none.
    ///
    /// # Errors
    ///
    /// As the index scan, for a value not of its slot's type, a probe of
    /// no index, the engine's refusal of the read (SQLCODE -911) or a
    /// failed read.
    pub(crate) fn read(&self, st: &State, tx: TxId, values: &[Value]) -> Result<Option<Row>> {
        let probe = match self.keyed(st, tx, placed(&self.probe, &self.slots, values)?, false)? {
            keyed::Probed::Read(found) => return Ok(found.map(|found| self.given(read_row(self.located, found)))),
            keyed::Probed::Unread(probe) => probe,
        };
        let Found { storage, ix, entries, projection, read, pruning } = probe_index(st, &self.table, &self.index, &probe, (false, true))?;
        read.record(&*storage, tx)?;
        let width = self.covering.then(|| st.catalog.table(&self.table).map(|t| t.columns.len())).flatten();
        let mut reading = Reading::of(&ix, Some(&self.needed), projection).pruned(pruning);
        // Each entry is counted and its read looked to for a cancel as the
        // scan does (RULES.md D44, D521).
        for entry in counted(entries, st.tx.stop_of(tx), |_| 1) {
            let (key, id) = entry?;
            // An entry of a version `tx` does not see finds no row
            // (`Index`); the one it sees is the only one of its key.
            if let Some(row) = entry_row(&*storage, (&ix, width), &mut reading, (tx, id), key)? {
                return Ok(Some(self.given(read_row(self.located, (id, row)))));
            }
        }
        Ok(None)
    }

    /// The row `tx` sees under the probe placed with `values`, with its
    /// id and all its columns read. It is the object row of a kept UPDATE
    /// or DELETE, read as `exec::row_entries` reads its plan's.
    ///
    /// # Errors
    ///
    /// As `read`.
    pub(crate) fn entry(&self, st: &State, tx: TxId, values: &[Value]) -> Result<Option<(RowId, Row)>> {
        let probe = match self.keyed(st, tx, placed(&self.probe, &self.slots, values)?, true)? {
            keyed::Probed::Read(found) => return Ok(found),
            keyed::Probed::Unread(probe) => probe,
        };
        let Found { storage, ix, entries, projection, read, pruning } = probe_index(st, &self.table, &self.index, &probe, (false, true))?;
        read.record(&*storage, tx)?;
        let mut reading = Reading::of(&ix, None, projection).pruned(pruning);
        for entry in counted(entries, st.tx.stop_of(tx), |_| 1) {
            let (key, id) = entry?;
            if let Some(found) = fetch(&*storage, &ix, &mut reading, (tx, id), &key)? {
                return Ok(Some(found));
            }
        }
        Ok(None)
    }

    /// Of `row` as read, the columns given.
    fn given(&self, mut row: Row) -> Row {
        let Some(outputs) = &self.outputs else { return row };
        let value = |held: &mut Row, c: usize| {
            let at = held.get_mut(c);
            let found = if self.distinct { at.map(|v| mem::replace(v, Value::Null)) } else { at.map(|v| v.clone()) };
            found.unwrap_or(Value::Null)
        };
        outputs.iter().map(|&c| value(&mut row, c)).collect()
    }
}

/// The index scan `plan` is, and the columns it gives of the scan's row.
/// The scan alone gives all of them, a projection of its columns those.
fn scan_of(plan: &Plan) -> Option<(Option<Vec<usize>>, &Plan)> {
    let given = match plan {
        Plan::Project { exprs, input, .. } => (Some(exprs.iter().map(column_of).collect::<Option<Vec<usize>>>()?), &**input),
        Plan::IndexScan { .. } => (None, plan),
        Plan::Filter { .. }
        | Plan::Limit { .. }
        | Plan::SeqScan { .. }
        | Plan::Nearest(_)
        | Plan::Product { .. }
        | Plan::HashJoin { .. }
        | Plan::IndexJoin { .. }
        | Plan::MergeJoin(_)
        | Plan::RangeJoin(_)
        | Plan::SemiJoin(_)
        | Plan::MarkJoin(_)
        | Plan::OuterJoin { .. }
        | Plan::Aggregate { .. }
        | Plan::Distinct { .. }
        | Plan::SetOp { .. }
        | Plan::System { .. }
        | Plan::Values { .. }
        | Plan::Append { .. }
        | Plan::MergeAppend { .. }
        | Plan::Sort { .. }
        | Plan::Recursive { .. }
        | Plan::Unnest(..)
        | Plan::Window { .. }
        | Plan::Polymorphic(..)
        | Plan::Recognize { .. }
        | Plan::WorkTable { .. }
        | Plan::Transition { .. }
        | Plan::Parallel(..)
        | Plan::Transfer(_)
        | Plan::Counted(_) => return None,
    };
    Some(given)
}

/// The column `e` gives as it is, if it is one.
fn column_of(e: &Expr) -> Option<usize> {
    if let Expr::Column(c) = e { Some(*c) } else { None }
}

// Unit tests of point reads found as their plans are (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
