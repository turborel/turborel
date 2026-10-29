//! \[DESIGN\] A projection of a table read a batch at a time (RULES.md D31):
//! of a scan, filtered or not, whose expressions read only columns the
//! scan reads, no subquery among them, of a table that reads by batch
//! (`TableStorage::reads_by_batch`), each row given made of the batch's
//! vectors, no row of the table's width made for it, a batch read ahead of
//! the rows given. A column or a constant is taken as it is; any other
//! expression is evaluated as a row's is (`exec/eval.rs`), of a row of
//! only the values it reads. Rows come as the scan gives batches,
//! in no particular order but the engine's; a projection of anything else
//! is made row by row (`exec.rs`). Split from batch.rs (RULES.md R-SIZE-2).

use std::sync::Arc;

use super::{Judging, scanned};
use crate::db::State;
use crate::error::Result;
use crate::exec::{RowIter, Runner, eval};
use crate::logical::{Expr, expr_columns, map_expr_columns};
use crate::physical::Plan;
use crate::storage::{Batch, Row};
use crate::txn::TxId;
use crate::value::Value;

/// What an expression of the projection gives: the value of the column at
/// this place of a batch, a constant, or an expression of a row of the
/// values at these places, its columns those of the row.
enum Place {
    Column(usize),
    Const(Value),
    Expr(Expr, Vec<usize>),
}

/// Whether `e` reads nothing but its row: no subquery, nor an outer
/// reference, in it or in the conditions of a CASE in it.
fn plain(e: &Expr) -> bool {
    let own = match e {
        Expr::Subquery(..) | Expr::Outer(..) => false,
        Expr::Case(whens, _) => whens.iter().all(|(c, _)| c.subqueries().is_empty()),
        Expr::Column(_) | Expr::Const(_) | Expr::Neg(_) | Expr::Arith(..) | Expr::Call(..) | Expr::Cast(..) | Expr::Domain(..) => true,
    };
    let mut all = own;
    e.for_each_child(&mut |child| all = all && plain(child));
    all
}

/// What `e`, of a scan of `columns`, gives of a batch, if it reads only
/// columns the scan reads.
fn place_of(e: &Expr, columns: &[usize]) -> Option<Place> {
    let at = |c: &usize| columns.iter().position(|x| x == c);
    match e {
        Expr::Column(c) => at(c).map(Place::Column),
        Expr::Const(v) => Some(Place::Const(v.clone())),
        Expr::Neg(_)
        | Expr::Arith(..)
        | Expr::Call(..)
        | Expr::Outer(..)
        | Expr::Case(..)
        | Expr::Cast(..)
        | Expr::Domain(..)
        | Expr::Subquery(..) => {
            if !plain(e) {
                return None;
            }
            let mut read = Vec::new();
            expr_columns(e, &mut read);
            read.sort_unstable();
            read.dedup();
            let inputs = read.iter().map(at).collect::<Option<Vec<usize>>>()?;
            let own = map_expr_columns(e.clone(), &|c| read.iter().position(|&r| r == c).unwrap_or(usize::MAX));
            Some(Place::Expr(own, inputs))
        }
    }
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
    /// The row an expression is evaluated of.
    scratch: Row,
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
    let places = exprs.iter().map(|e| place_of(e, columns)).collect::<Option<Vec<_>>>()?;
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
            scratch: Vec::new(),
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
        let value = |at: &usize| self.batch.columns.get(*at).and_then(|v| v.get(r)).unwrap_or(&Value::Null);
        let mut row = Vec::with_capacity(self.places.len());
        for place in &self.places {
            row.push(match place {
                Place::Column(at) => value(at).clone(),
                Place::Const(v) => v.clone(),
                // Its values in a row kept, each written over.
                Place::Expr(e, inputs) => {
                    self.scratch.resize(inputs.len(), Value::Null);
                    for (slot, at) in self.scratch.iter_mut().zip(inputs) {
                        slot.clone_from(value(at));
                    }
                    eval(e, &self.scratch, &self.runner)?
                }
            });
        }
        Ok(Some(row))
    }
}

impl<B: Iterator<Item = Result<Batch>>> Iterator for Projected<B> {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Self::Item> {
        self.step().transpose()
    }
}
