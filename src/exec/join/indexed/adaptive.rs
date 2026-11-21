//! A bounded small input can replace repeated spatial index probes with
//! one visible table scan. Cost estimates choose work only, never results.
//! Capacity or memory refusal resumes the existing probe iterator without
//! rereading the buffered input. Exact predicates and ON residuals remain.

mod held;
mod observed;
mod scan;
mod spilled;

use super::{IndexJoin, RowIter, failed, refined::Binding};
use crate::db::State;
use crate::error::Result;
use crate::exec::Db;
use crate::spatial::{
    masks::{AXES, ROWS},
    packed::Packed,
};
use crate::storage::cost_of;
use held::Held;

pub(super) fn start(join: IndexJoin, columns: Vec<usize>, db: &Db) -> RowIter {
    Box::new(observed::Observed::new(join, columns, db))
}

fn chosen(mut join: IndexJoin, mut columns: Vec<usize>, db: &Db) -> Result<RowIter> {
    let Some(mut held) = Held::new(db) else { return Ok(Box::new(join)) };
    let complete = held.read(&mut join.left)?;
    if !complete {
        return if held.outgrown() { spilled::chosen(join, held, columns, db) } else { Ok(held.resume(join)) };
    }
    let Some(width) = held.rows.first().map(Vec::len) else { return Ok(Box::new(std::iter::empty())) };
    let Some(binding) = Binding::of(&join, width) else { return Ok(held.resume(join)) };
    if !columns.contains(&binding.inner) {
        columns.push(binding.inner);
    }
    let scan = cheaper(&join, &held, &columns, db)?;
    if !scan {
        return Ok(held.resume(join));
    }
    let bounds = held.bounds(binding.outer)?;
    let packed = Packed::new(&bounds)?;
    scan::open(join, held, (binding, packed), &columns)
}

fn cheaper(join: &IndexJoin, held: &Held, columns: &[usize], db: &State) -> Result<bool> {
    let storage = &join.table.storage;
    let rows = cost_of(storage.row_count());
    let model = db.cost();
    let build = cost_of(held.rows.len());
    // Charge all leaf searches before relying on hierarchy pruning. Exact
    // predicate work applies to either candidate path. This deliberately
    // leaves selective joins on their probes when the scan benefit is small.
    let leaves = cost_of(held.rows.len().div_ceil(ROWS).checked_next_power_of_two().unwrap_or(usize::MAX));
    let compared = leaves * cost_of(AXES) * model.sort_compare * (build.min(cost_of(ROWS)) + 1.0_f64).log2();
    let scanned = storage.scan_cost(columns) + rows * compared;
    let mut probes = 0.0_f64;
    for row in &held.rows {
        if let Some(stop) = &join.table.stop {
            stop.check()?;
        }
        let Some(probe) = join.probe.bind(row, join.table.ix.columns.len())? else { continue };
        let Some(matches) = join.table.index.estimate(&probe) else { return Ok(false) };
        probes += model.index_descent * (rows + 1.0_f64).log2() + cost_of(matches) * storage.fetch_cost();
    }
    Ok(scanned < probes)
}

#[cfg(test)]
mod tests;
