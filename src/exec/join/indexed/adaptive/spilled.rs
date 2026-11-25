//! \[DESIGN\] An adaptive spatial join whose outer input outgrows its work
//! area (RULES.md D38, D138). The outer rows are written to a run of the
//! spill area. Probes priced from the rows held go on, reading the run
//! back, unless one scan of the table costs less. Then both inputs are
//! partitioned by a grid over the outer envelopes' hull (`spilled/grid.rs`),
//! each shape written to every cell it spans, and the cells joined one at
//! a time, the outer rows of each held a work area's worth at a time while
//! its table rows are read. A pair is produced only in the cell of its
//! reference point, so each pair once. NULL and empty geometries, and
//! table rows apart from the hull, are written nowhere, as they meet
//! nothing. The exact relationship and the rest of the condition judge
//! every candidate, as in memory, so the results are the same.

mod grid;
mod joined;

use std::sync::Arc;

use super::{AXES, Binding, Held, IndexJoin, RowIter, cost_of};
use crate::db::State;
use crate::error::Result;
use crate::exec::Db;
use crate::exec::{broken, work::Work};
use crate::geometry::Geometry;
use crate::memory::MemoryBudget;
use crate::spatial::Bounds;
use crate::spill::{SpillRun, record_row, row_bytes, row_record};
use crate::storage::{Row, located};
use crate::txn::checked;
use grid::Grid;

/// The outer rows written, how many, their bytes held in memory, and the
/// hull of their envelopes.
struct Outer {
    run: Box<dyn SpillRun>,
    count: usize,
    bytes: usize,
    hull: Option<Bounds>,
}

impl Outer {
    fn add(&mut self, row: &Row, column: usize) -> Result<()> {
        if let Some(bounds) = bounds_of(row, column)? {
            self.hull = Some(self.hull.map_or(bounds, |hull| hull.union(bounds)));
        }
        self.count = self.count.saturating_add(1);
        self.bytes = self.bytes.saturating_add(row_bytes(row));
        self.run.push(&row_record(row))
    }
}

/// `join` with its outer input, of which `held` holds what fit, joined by
/// probes or by partitions, as costs less; by probes if no run can be made.
pub(super) fn chosen(mut join: IndexJoin, held: Held, mut columns: Vec<usize>, db: &Db) -> Result<RowIter> {
    let Some(binding) = held.rows.first().and_then(|row| Binding::of(&join, row.len())) else { return Ok(held.resume(join)) };
    let mut work = Work::of(db, join.table.tx);
    let Ok(run) = work.run() else { return Ok(held.resume(join)) };
    let each = probe_cost(&join, (&held.rows, columns.len()), db)?;
    let mut outer = Outer { run, count: 0, bytes: 0, hull: None };
    let (rows, overflow, reservation) = held.into_parts();
    for row in rows.into_iter().chain(overflow) {
        outer.add(&row, binding.outer)?;
    }
    drop(reservation);
    let left = std::mem::replace(&mut join.left, Box::new(std::iter::empty()));
    for row in left {
        outer.add(&row?, binding.outer)?;
    }
    if !columns.contains(&binding.inner) {
        columns.push(binding.inner);
    }
    let scanned = scan_cost(&join, &outer, &columns, db);
    let scan = each.is_some_and(|each| scanned < each * cost_of(outer.count));
    if scan {
        return partitioned(join, (outer, binding), &columns, (work, Arc::clone(&db.memory)));
    }
    let types = work.types().clone();
    join.left = Box::new(outer.run.into_records()?.map(move |record| record_row(&record?, &types)));
    Ok(Box::new(join))
}

/// The mean cost of a probe of the rows held, each match fetched of
/// `read` columns, as `cheaper` prices them; none if the index cannot
/// estimate one.
fn probe_cost(join: &IndexJoin, (rows, read): (&[Row], usize), db: &State) -> Result<Option<f64>> {
    let (model, table) = (db.cost(), cost_of(join.table.storage.row_count()));
    let mut total = 0.0_f64;
    for row in rows {
        let Some(probe) = join.probe.bind(row, join.table.ix.columns.len())? else { continue };
        let Some(matches) = join.table.index.estimate(&probe) else { return Ok(None) };
        total += model.index_descent * (table + 1.0_f64).log2() + cost_of(matches) * join.table.storage.fetch_cost(read);
    }
    Ok(Some(total / cost_of(rows.len().max(1))))
}

/// One scan of the table, each of its rows searched among the outer
/// rows, and both inputs written and read again.
fn scan_cost(join: &IndexJoin, outer: &Outer, columns: &[usize], db: &State) -> f64 {
    let (model, storage) = (db.cost(), &join.table.storage);
    let rows = cost_of(storage.row_count());
    let searched = rows * cost_of(AXES) * model.sort_compare * (cost_of(outer.count) + 1.0_f64).log2();
    storage.scan_cost(columns) + searched + (rows + cost_of(outer.count)) * model.sort_compare
}

/// Both inputs written to the cells of a grid over the outer hull, and
/// the cells joined in turn.
fn partitioned(
    join: IndexJoin,
    (outer, binding): (Outer, Binding),
    columns: &[usize],
    (mut work, memory): (Work, Arc<MemoryBudget>),
) -> Result<RowIter> {
    let Some(hull) = outer.hull else { return Ok(Box::new(std::iter::empty())) };
    let most = work.width().checked_div(2).unwrap_or(1).min(work.partitions()).max(1);
    let wanted = outer.bytes.saturating_mul(2).div_ceil(work.limit().max(1)).max(1);
    let grid = Grid::new(hull, wanted.min(most));
    let mut outers = runs(&mut work, grid.count())?;
    let types = work.types().clone();
    for record in outer.run.into_records()? {
        let record = record?;
        let Some(bounds) = bounds_of(&record_row(&record, &types)?, binding.outer)? else { continue };
        for cell in grid.cells(bounds) {
            outers.get_mut(cell).ok_or_else(|| broken("a spatial partition is missing"))?.push(&record)?;
        }
    }
    let mut inners = runs(&mut work, grid.count())?;
    let (stop, read) = (join.table.stop.clone(), join.table.located);
    let rows = checked(Arc::clone(&join.table.storage).scan(join.table.tx, columns), stop);
    for row in rows {
        let (id, mut row) = row?;
        if read {
            row.push(located(id));
        }
        let Some(bounds) = bounds_of(&row, binding.inner)?.filter(|bounds| bounds.intersects(hull)) else { continue };
        let record = row_record(&row);
        for cell in grid.cells(bounds) {
            inners.get_mut(cell).ok_or_else(|| broken("a spatial partition is missing"))?.push(&record)?;
        }
    }
    let cells = outers.into_iter().zip(inners).enumerate().collect();
    Ok(Box::new(joined::Joined::new(join, (binding, grid), cells, (work, memory))))
}

fn runs(work: &mut Work, count: usize) -> Result<Vec<Box<dyn SpillRun>>> {
    (0..count).map(|_| work.run()).collect()
}

/// The envelope of `row`'s geometry at `column`, none if it is null or
/// empty, which meets nothing.
fn bounds_of(row: &Row, column: usize) -> Result<Option<Bounds>> {
    let value = row.get(column).ok_or_else(|| broken("a spatial join column is missing"))?;
    if value.is_null() { Ok(None) } else { Geometry::from_value(value).map(|geometry| geometry.bounds()) }
}
