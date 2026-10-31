//! \[DESIGN\] Joins of streams, a batch at a time (RULES.md D31): an inner
//! hash join, an outer join [ISO/IEC 9075:1992 §7.5] or a product
//! [X3.135-1986 §5.20 GR2b], of two subtrees that give batches, each column
//! the join reads of their rows to match them among those their batches
//! hold.
//!
//! Each reads its right operand first, as the row path does, of its
//! batches' rows if its tables read by batch, and holds it as the row path
//! holds it; each of its left operand's batches then probes it, or is
//! paired with it, a batch of the joined rows made at a time
//! (`joined/pairs.rs`): their columns the left's, gathered from the batch
//! probing as vectors of their kind, then the right's. An inner hash join
//! and a LEFT one of no raising condition probe a built right
//! (`joined/probe.rs`), an inner one's partitions past the work area each
//! in turn (`joined/parted.rs`); any other outer join probes a right held
//! with a flag of each row, whether it matched (`joined/outer.rs`); a
//! product pairs batches (`joined/nested.rs`). An outer join's right
//! operand past the work area is joined as the row path joins it
//! (`exec/join/outer.rs`) and its rows gathered into batches.
//!
//! A left operand whose rows are asked for in an order (`in_order`) is read
//! in it, its rows gathered into batches, and every join of it gives its
//! rows in it.

mod nested;
mod outer;
mod pairs;
mod parted;
mod probe;

use std::sync::Arc;

use super::stream::{self, Batches};
use crate::db::State;
use crate::error::Result;
use crate::exec::join::Joining;
use crate::exec::{RowIter, execute, failed};
use crate::logical::{OuterJoin, cond_columns};
use crate::physical::Plan;
use crate::storage::{Row, batches_of};
use crate::txn::{TxId, checked};
use crate::value::Value;
use probe::Kind;

/// The operands of `plan`, if it is a join: a product, a hash join or an
/// outer join.
pub(super) fn operands(plan: &Plan) -> Option<(&Plan, &Plan)> {
    match plan {
        Plan::Product { left, right } | Plan::HashJoin { left, right, .. } | Plan::OuterJoin { left, right, .. } => Some((left, right)),
        Plan::SeqScan { .. }
        | Plan::IndexScan { .. }
        | Plan::Filter { .. }
        | Plan::IndexJoin { .. }
        | Plan::Aggregate { .. }
        | Plan::Project { .. }
        | Plan::Append { .. }
        | Plan::SetOp { .. }
        | Plan::Values { .. }
        | Plan::System { .. }
        | Plan::Sort { .. }
        | Plan::Distinct { .. } => None,
    }
}

/// The columns of the join `plan`'s rows its batches hold, and the width
/// of its rows, if it is a join of streams (`stream::layout`) every column
/// of whose keys and conditions their batches hold.
pub(super) fn layout(plan: &Plan, db: &State) -> Option<(Vec<usize>, usize)> {
    let (left, right) = operands(plan)?;
    let (mut columns, left_width) = stream::layout(left, db)?;
    let (right_columns, right_width) = stream::layout(right, db)?;
    let held = |c: &usize| c.checked_sub(left_width).map_or_else(|| columns.contains(c), |r| right_columns.contains(&r));
    if !read(plan, left_width).iter().all(held) {
        return None;
    }
    columns.extend(right_columns.iter().map(|&c| c.saturating_add(left_width)));
    Some((columns, left_width.saturating_add(right_width)))
}

/// The columns of the pairs of its operands' rows the join `plan`, whose
/// left operand is `width` columns wide, reads to match them: of its
/// keys, its condition, and its operands' own conditions.
fn read(plan: &Plan, width: usize) -> Vec<usize> {
    let mut columns = Vec::new();
    let keys = match plan {
        Plan::HashJoin { keys, .. } => keys,
        Plan::OuterJoin { keys, cond, own, .. } => {
            let (left_own, right_own) = own.as_deref().map_or((None, None), |(l, r)| (l.as_ref(), r.as_ref()));
            for c in [cond.as_ref(), left_own].into_iter().flatten() {
                cond_columns(c, &mut columns);
            }
            let mut of_right = Vec::new();
            if let Some(c) = right_own {
                cond_columns(c, &mut of_right);
            }
            columns.extend(of_right.into_iter().map(|c| c.saturating_add(width)));
            keys
        }
        Plan::Product { .. }
        | Plan::SeqScan { .. }
        | Plan::IndexScan { .. }
        | Plan::Filter { .. }
        | Plan::IndexJoin { .. }
        | Plan::Aggregate { .. }
        | Plan::Project { .. }
        | Plan::Append { .. }
        | Plan::SetOp { .. }
        | Plan::Values { .. }
        | Plan::System { .. }
        | Plan::Sort { .. }
        | Plan::Distinct { .. } => return columns,
    };
    columns.extend(keys.iter().flat_map(|&(l, r)| [l, r.saturating_add(width)]));
    columns
}

/// The batches of the join `plan`, one of streams (`layout`), each made
/// only while the statement is not cancelled (RULES.md D44): a join of
/// operands held in memory reads no table as it makes them.
///
/// # Errors
///
/// A table's storage missing; the right operand's errors.
pub(super) fn batches(plan: Plan, db: &Arc<State>, tx: TxId) -> Result<Batches> {
    let stop = db.tx.stop_of(tx);
    Ok(Box::new(checked(joined(plan, db, tx)?, stop)))
}

/// `batches`' work: the join's batches.
fn joined(plan: Plan, db: &Arc<State>, tx: TxId) -> Result<Batches> {
    match plan {
        Plan::Product { left, right } => nested::multiplied((*left, *right), db, tx),
        Plan::HashJoin { left, right, keys, .. } => probe::probed((*left, *right, keys), Kind::Inner, db, tx),
        Plan::OuterJoin { kind, left, right, keys, cond, own, widths, .. } => {
            if kind == OuterJoin::Left && own.is_none() && cond.as_ref().is_none_or(|c| !c.can_raise()) {
                return probe::probed((*left, *right, keys), Kind::Left(cond, widths), db, tx);
            }
            outer::joined(Joining { kind, keys, cond, own, widths }, (*left, *right), db, tx)
        }
        Plan::SeqScan { .. }
        | Plan::IndexScan { .. }
        | Plan::Filter { .. }
        | Plan::IndexJoin { .. }
        | Plan::Aggregate { .. }
        | Plan::Project { .. }
        | Plan::Append { .. }
        | Plan::SetOp { .. }
        | Plan::Values { .. }
        | Plan::System { .. }
        | Plan::Sort { .. }
        | Plan::Distinct { .. } => Err(crate::exec::broken("a join of streams is not one")),
    }
}

/// The rows of `plan`, a subtree that gives batches: made of them, each of
/// its width, its columns the batches do not hold null, if every table it
/// reads reads by batch; else read row by row, as a table that makes each
/// row of a batch gives them as fast so.
fn operand_rows(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    if !stream::reads_by_batch(&plan, db) {
        return execute(plan, db, tx);
    }
    let (columns, width) = stream::layout(&plan, db).unwrap_or_default();
    match stream::batches(plan, db, tx) {
        Ok(batches) => rows_of(batches, columns, width),
        Err(e) => failed(e),
    }
}

/// The rows of `batches`, those a filter kept of each, each of `width`
/// columns, the values of a batch's vectors at `columns`, the others null.
fn rows_of(batches: Batches, columns: Vec<usize>, width: usize) -> RowIter {
    Box::new(batches.flat_map(move |kept| match kept {
        Ok((batch, rows)) => {
            let made: Vec<Result<Row>> = rows
                .unwrap_or_else(|| (0..batch.rows).collect())
                .into_iter()
                .map(|r| {
                    let mut row = vec![Value::Null; width];
                    for (at, &c) in columns.iter().enumerate() {
                        if let Some(slot) = row.get_mut(c) {
                            *slot = batch.cell(at, r).to_value();
                        }
                    }
                    Ok(row)
                })
                .collect();
            made
        }
        Err(e) => vec![Err(e)],
    }))
}

/// `rows`, joined, gathered into batches of their values of `columns`.
pub(super) fn gathered(rows: RowIter, columns: &[usize]) -> Batches {
    // Rows with no id, which a batch does not keep.
    Box::new(batches_of(Box::new(rows.map(|r| r.map(|row| (0, row)))), columns).map(|b| b.map(|b| (b, None))))
}

/// The batches of `plan`, a subtree that gives them, a join's left
/// operand: of its rows in the order asked of them, if one is
/// (`in_order`), gathered; else as it gives them.
///
/// # Errors
///
/// As `stream::batches`.
fn left_batches(plan: Plan, db: &Arc<State>, tx: TxId) -> Result<Batches> {
    if !in_order(&plan) {
        return stream::batches(plan, db, tx);
    }
    let columns = stream::layout(&plan, db).map(|l| l.0).unwrap_or_default();
    Ok(gathered(execute(plan, db, tx), &columns))
}

/// Whether the rows of `plan`, a subtree that gives batches, are asked for
/// in an order, as the planner relies on (`physical.rs`) where a batch
/// comes in none (RULES.md D31): a scan's in its table's
/// (`TableStorage::scan_ordered`), a join's in its left operand's. A plan
/// that gives no batches is taken to be, so its order is kept.
fn in_order(plan: &Plan) -> bool {
    match plan {
        Plan::SeqScan { order, .. } => !order.is_empty(),
        Plan::Filter { input, .. } | Plan::Project { input, .. } => in_order(input),
        Plan::Product { left, .. } | Plan::HashJoin { left, .. } | Plan::OuterJoin { left, .. } => in_order(left),
        Plan::Append { left, right } => in_order(left) || in_order(right),
        Plan::IndexScan { .. }
        | Plan::IndexJoin { .. }
        | Plan::Aggregate { .. }
        | Plan::SetOp { .. }
        | Plan::Values { .. }
        | Plan::System { .. }
        | Plan::Sort { .. }
        | Plan::Distinct { .. } => true,
    }
}

// Unit tests: operands of joins no statement plans as streams (RULES.md
// R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::QualifiedName;

    /// Verifies: [DESIGN] RULES.md D31: whether a join's left operand is
    /// asked for its rows in an order: a scan in none is not; the rows of
    /// two streams, one after the other, are if either is; a plan that
    /// gives no batches is taken to be, so its order is kept.
    #[test]
    fn rows_asked_for_in_an_order() {
        let scan = || {
            let table = QualifiedName { auth: "A".into(), ident: "T".into() };
            Box::new(Plan::SeqScan { table, columns: Vec::new(), rows: 0_usize, cost: 0.0_f64, order: Vec::new() })
        };
        let values = || Box::new(Plan::Values { rows: Vec::new(), detail: String::new() });
        assert!(!in_order(&scan()));
        assert!(in_order(&Plan::Append { left: scan(), right: values() }));
        assert!(in_order(&Plan::Append { left: values(), right: scan() }));
        assert!(!in_order(&Plan::Append { left: scan(), right: scan() }));
    }
}
