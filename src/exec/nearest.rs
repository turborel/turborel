//! Best-first geometry candidates refined against visible rows (D15,
//! D29, D38, D44, D85). A strict lower-bound cutoff retains distance ties;
//! the existing sort decides their secondary order. An uncertified index
//! or an unaffordable threshold heap falls back before producing rows.
//! An approximate index's read is `nearest/approx.rs` (D94).

mod approx;
mod threshold;

use std::sync::Arc;

use super::entries::{Reading, fetch};
use super::{RowIter, Runner, Subqueries, broken, eval, eval_cond, failed};
use crate::catalog::{IndexDesc, KeyShape};
use crate::db::State;
use crate::error::Result;
use crate::index::{Key, NeighbourEntries, Probe};
use crate::logical::Truth;
use crate::physical::{NearestRead, Plan};
use crate::storage::{Condition, Row, RowId, TableStorage};
use crate::txn::{TxId, checked};
use crate::value::Value;
use threshold::Threshold;

#[inline(never)]
pub(super) fn nearest(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    let Plan::Nearest(read) = plan else { return failed(broken("a nearest read is not one")) };
    match open(*read, db, tx) {
        Ok(rows) => rows,
        Err(error) => failed(error),
    }
}

fn open(read: NearestRead, db: &Arc<State>, tx: TxId) -> Result<RowIter> {
    if let Some(stop) = db.tx.stop_of(tx) {
        stop.check()?;
    }
    let storage = Arc::clone(db.storage_of(&read.table)?);
    let Some(mut query) = read.query() else { return Err(broken("a nearest read has no constant distance operand")) };
    // A score's bounds and its calls read one snapshot (D87).
    if read.approximate.is_some() {
        return approx::approximate(read, storage, db, (tx, query));
    }
    let runner = Runner::new(db, tx);
    if read.descending {
        query.statistics = runner.ranking_statistics(&read.index)?;
    }
    let Some(threshold) = Threshold::new(read.first, db).map(|t| t.negated(read.descending)) else {
        return Ok(scanned(read, storage, db, tx, false));
    };
    let index = Arc::clone(db.index_of(&read.index)?);
    let key = db.catalog.index(&read.index).map(IndexDesc::key_shape).ok_or_else(|| broken("a nearest index has no definition"))?;
    storage.read_by_condition(tx, &Condition { columns: Vec::new(), probe: Probe::All })?;
    let projection = index.key_projection();
    let Some(entries) = index.nearest(&query, &db.memory)? else { return Ok(scanned(read, storage, db, tx, false)) };
    let reading = Reading::of(&key, Some(&read.columns), projection);
    let entries = Box::new(checked(entries, db.tx.stop_of(tx)));
    let rows = Search { read, storage, key, reading, entries, threshold, runner, db: Arc::clone(db), tx, ended: false, tail: None };
    Ok(Box::new(checked(rows, db.tx.stop_of(tx))))
}

/// The fallback computes the same filter and measure lazily. After an
/// exhausted candidate traversal, only NULL distances remain to add.
fn scanned(read: NearestRead, storage: Arc<dyn TableStorage>, db: &Arc<State>, tx: TxId, nulls: bool) -> RowIter {
    let runner = Runner::new(db, tx);
    let entries = checked(storage.scan(tx, &read.columns), db.tx.stop_of(tx));
    let rows = entries.filter_map(move |entry| {
        entry
            .and_then(|entry| measured(&read, entry, &runner))
            .and_then(|row| {
                let Some(row) = row else { return Ok(None) };
                let distance = row.last().ok_or_else(|| broken("a nearest row has no distance"))?;
                Ok((!nulls || distance.is_null()).then_some(row))
            })
            .transpose()
    });
    Box::new(checked(rows, db.tx.stop_of(tx)))
}

fn measured(read: &NearestRead, (id, mut row): (RowId, Row), runner: &Runner) -> Result<Option<Row>> {
    if read.located {
        row.push(crate::storage::located(id));
    }
    if let Some(condition) = &read.condition {
        let truth = eval_cond(condition, &row, runner)?;
        if truth != Truth::True {
            return Ok(None);
        }
    }
    let distance = eval(&read.measure, &row, runner)?;
    row.push(distance);
    Ok(Some(row))
}

struct Search {
    read: NearestRead,
    storage: Arc<dyn TableStorage>,
    key: KeyShape,
    reading: Reading,
    entries: NeighbourEntries,
    threshold: Threshold,
    runner: Runner,
    db: Arc<State>,
    tx: TxId,
    ended: bool,
    tail: Option<RowIter>,
}

impl Search {
    fn candidate(&mut self, key: &Key, id: RowId) -> Result<Option<Row>> {
        let Some(entry) = fetch(&*self.storage, &self.key, &mut self.reading, (self.tx, id), key)? else { return Ok(None) };
        let Some(row) = measured(&self.read, entry, &self.runner)? else { return Ok(None) };
        match row.last() {
            Some(Value::Null) => Ok(None),
            Some(Value::Approx(distance)) => {
                // A score's bound is of its negation (`NearestQuery`).
                self.threshold.add(if self.read.descending { -*distance } else { *distance })?;
                Ok(Some(row))
            }
            Some(_) | None => Err(broken("a nearest distance is neither DOUBLE PRECISION nor NULL")),
        }
    }

    fn exhausted(&mut self) -> Option<Result<Row>> {
        self.ended = true;
        if !self.threshold.full() {
            self.tail = Some(scanned(self.read.clone(), Arc::clone(&self.storage), &self.db, self.tx, true));
        }
        self.tail.as_mut().and_then(Iterator::next)
    }
}

impl Iterator for Search {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.ended {
            return self.tail.as_mut().and_then(Iterator::next);
        }
        loop {
            let Some(entry) = self.entries.next() else { return self.exhausted() };
            let outcome = entry.and_then(|(bound, key, id)| {
                if self.threshold.beyond(bound)? {
                    self.ended = true;
                    return Ok(None);
                }
                self.candidate(&key, id)
            });
            match outcome {
                Ok(Some(row)) => return Some(Ok(row)),
                Ok(None) => {
                    if self.ended {
                        return None;
                    }
                }
                Err(error) => {
                    self.ended = true;
                    return Some(Err(error));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
