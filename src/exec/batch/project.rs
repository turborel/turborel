//! \[DESIGN\] A projection of a table read a batch at a time (RULES.md D31):
//! of a scan, filtered or not, whose every expression is a column the scan
//! reads or a constant, of a table that reads by batch
//! (`TableStorage::reads_by_batch`), each row given made of the batch's
//! vectors, no row of the table's width made for it, a batch read ahead of
//! the rows given. Rows come as the scan gives batches,
//! in no particular order but the engine's; a projection of anything else
//! is made row by row (`exec.rs`). Split from batch.rs (RULES.md R-SIZE-2).

use std::sync::Arc;

use super::{Judging, scanned};
use crate::db::State;
use crate::error::Result;
use crate::exec::{RowIter, Runner};
use crate::logical::Expr;
use crate::physical::Plan;
use crate::storage::{Batch, Row};
use crate::txn::TxId;
use crate::value::Value;

/// What an expression of the projection gives: the value of the column at
/// this place of a batch, or a constant.
enum Place {
    Column(usize),
    Const(Value),
}

/// A projection's rows, made of the batches as they are read.
struct Projected<B> {
    batches: B,
    places: Vec<Place>,
    judging: Option<Judging>,
    runner: Runner,
    batch: Batch,
    /// The rows of the batch the filter kept, and the next to give.
    kept: Vec<usize>,
    next: usize,
}

/// The rows of the projection `plan`, read by batches, if it is one read
/// so; else none, for it to be made row by row.
pub(in crate::exec) fn projected(plan: &Plan, db: &Arc<State>, tx: TxId) -> Option<RowIter> {
    let Plan::Project { exprs, input, .. } = plan else { return None };
    let (scan, filter) =
        if let Plan::Filter { cond, input, .. } = input.as_ref() { (input.as_ref(), Some(cond)) } else { (input.as_ref(), None) };
    let Plan::SeqScan { table, columns, .. } = scan else { return None };
    // A table that makes each row of a batch gives rows as fast read one at
    // a time, without reading ahead.
    if !db.storage_of(table).is_ok_and(|s| s.reads_by_batch()) {
        return None;
    }
    let places = exprs
        .iter()
        .map(|e| match e {
            Expr::Column(c) => columns.iter().position(|x| x == c).map(Place::Column),
            Expr::Const(v) => Some(Place::Const(v.clone())),
            Expr::Neg(_)
            | Expr::Arith(..)
            | Expr::Call(..)
            | Expr::Outer(..)
            | Expr::Case(..)
            | Expr::Cast(..)
            | Expr::Domain(..)
            | Expr::Subquery(..) => None,
        })
        .collect::<Option<Vec<_>>>()?;
    let judging = filter.map(|cond| Judging::of(cond, columns));
    Some(match scanned((table, columns, filter), db, tx) {
        Ok(batches) => Box::new(Projected {
            batches,
            places,
            judging,
            runner: Runner::new(db, tx),
            batch: Batch::default(),
            kept: Vec::new(),
            next: 0,
        }),
        Err(e) => crate::exec::failed(e),
    })
}

impl<B: Iterator<Item = Result<Batch>>> Projected<B> {
    /// The next row given, if any is left.
    fn step(&mut self) -> Result<Option<Row>> {
        while self.next >= self.kept.len() {
            let Some(batch) = self.batches.next().transpose()? else { return Ok(None) };
            self.kept = match &self.judging {
                Some(judging) => judging.kept(&batch, &self.runner)?,
                None => (0..batch.rows).collect(),
            };
            (self.batch, self.next) = (batch, 0);
        }
        let r = self.kept.get(self.next).copied().unwrap_or_default();
        self.next = self.next.saturating_add(1);
        Ok(Some(
            self.places
                .iter()
                .map(|p| match p {
                    Place::Column(at) => self.batch.columns.get(*at).and_then(|v| v.get(r)).cloned().unwrap_or(Value::Null),
                    Place::Const(v) => v.clone(),
                })
                .collect(),
        ))
    }
}

impl<B: Iterator<Item = Result<Batch>>> Iterator for Projected<B> {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Self::Item> {
        self.step().transpose()
    }
}
