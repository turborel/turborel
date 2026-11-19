//! The cells of a partitioned spatial join, joined one at a time (RULES.md
//! D38, D138). A cell's outer rows are held a work area's worth at a time,
//! reserved of the memory budget, in the hierarchy the in-memory scan
//! builds (`spatial/packed.rs`); its table rows are read once for each
//! such part. Each candidate pair is kept only in the cell of its
//! reference point and judged as in memory (`adaptive/scan.rs`).

use std::sync::Arc;

use super::super::held::metadata;
use super::super::scan::joined;
use super::super::{Binding, IndexJoin};
use super::{Grid, bounds_of};
use crate::error::Result;
use crate::exec::project::Columns;
use crate::exec::{Runner, broken, work::Work};
use crate::functions::ScalarRef;
use crate::memory::{MemoryBudget, Reservation};
use crate::spatial::{
    Bounds,
    packed::{Cursor, Packed},
};
use crate::spill::{Records, SpillRun, record_row, row_bytes};
use crate::storage::Row;
use crate::txn::Stop;
use crate::value::types::TypeRegistry;

/// A cell's number and its outer and table rows' runs.
type Runs = (usize, (Box<dyn SpillRun>, Box<dyn SpillRun>));

/// What every cell's pairs are judged and held by.
struct Matching {
    binding: Binding,
    grid: Grid,
    runner: Runner,
    output: Option<Columns>,
    /// Scalar implementations prepared for each outer row, as in memory.
    prepared: Option<Arc<MemoryBudget>>,
    memory: Arc<MemoryBudget>,
    types: TypeRegistry,
    limit: usize,
}

/// The join of the cells not yet joined.
pub(super) struct Joined {
    cells: std::vec::IntoIter<Runs>,
    cell: Option<Cell>,
    matching: Matching,
    stop: Option<Arc<Stop>>,
    /// Its spill area, which its runs are of.
    _work: Work,
}

impl Joined {
    pub(super) fn new(
        join: IndexJoin,
        (binding, grid): (Binding, Grid),
        cells: Vec<Runs>,
        (work, memory): (Work, Arc<MemoryBudget>),
    ) -> Joined {
        let matching = Matching {
            binding,
            grid,
            runner: join.runner,
            output: join.output,
            prepared: join.budget,
            memory,
            types: work.types().clone(),
            limit: work.limit(),
        };
        Joined { cells: cells.into_iter(), cell: None, matching, stop: join.table.stop, _work: work }
    }

    fn step(&mut self) -> Result<Option<Row>> {
        loop {
            if let Some(stop) = &self.stop {
                stop.check()?;
            }
            let Some(cell) = self.cell.as_mut() else {
                let Some((at, (outer, inner))) = self.cells.next() else { return Ok(None) };
                self.cell = Some(Cell { at, outer: outer.into_records()?, pending: None, inner, part: None, rows: None, probe: None });
                continue;
            };
            match cell.next(&self.matching)? {
                Some(row) => return Ok(Some(row)),
                None => self.cell = None,
            }
        }
    }
}

impl Iterator for Joined {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Self::Item> {
        self.step().transpose()
    }
}

/// The outer rows of a cell held at once, their envelopes, the functions
/// prepared for them, and their hierarchy.
struct Part {
    rows: Vec<Row>,
    bounds: Vec<Option<Bounds>>,
    functions: Vec<ScalarRef>,
    packed: Packed,
    _held: Reservation,
}

/// A cell being joined: its outer rows not yet held, the one read past
/// the last part, its table rows' run, the part held, the pass over its
/// table rows, and the table row being matched.
struct Cell {
    at: usize,
    outer: Records,
    pending: Option<Row>,
    inner: Box<dyn SpillRun>,
    part: Option<Part>,
    rows: Option<Records>,
    probe: Option<(Row, Bounds, Cursor)>,
}

impl Cell {
    /// Its next pair, none once its outer rows are all matched.
    fn next(&mut self, m: &Matching) -> Result<Option<Row>> {
        loop {
            if let Some((row, bounds, cursor)) = &mut self.probe {
                let part = self.part.as_ref().ok_or_else(|| broken("a spatial partition part is missing"))?;
                let Some(at) = part.packed.next(*bounds, cursor)? else {
                    self.probe = None;
                    continue;
                };
                let (Some(outer), Some(Some(envelope)), Some(function)) = (part.rows.get(at), part.bounds.get(at), part.functions.get(at))
                else {
                    return Err(broken("a spatial partition candidate is missing"));
                };
                if !m.grid.owns(self.at, *envelope, *bounds) {
                    continue;
                }
                if let Some(pair) = joined((&m.binding, &m.runner, m.output.as_ref()), (outer, row), function)? {
                    return Ok(Some(pair));
                }
            } else if let Some(rows) = &mut self.rows {
                let Some(record) = rows.next().transpose()? else {
                    self.rows = None;
                    continue;
                };
                let row = record_row(&record, &m.types)?;
                if let Some(bounds) = bounds_of(&row, m.binding.inner)? {
                    self.probe = Some((row, bounds, Cursor::default()));
                }
            } else {
                let Some(part) = self.part(m)? else { return Ok(None) };
                self.part = Some(part);
                self.rows = Some(self.inner.records()?);
            }
        }
    }

    /// The next part of its outer rows: as many as the work area and the
    /// memory budget hold, at least one; none once all are read.
    fn part(&mut self, m: &Matching) -> Result<Option<Part>> {
        self.part = None;
        let mut held = m.memory.reserve(0, "a part of a spatial join partition")?;
        let (mut rows, mut payload) = (Vec::new(), 0_usize);
        while let Some(row) = self.next_outer(m)? {
            payload = payload.saturating_add(row_bytes(&row));
            let bytes = metadata(rows.len().saturating_add(1)).saturating_add(payload);
            let fits = bytes <= m.limit && held.resize(bytes).is_ok();
            if !fits && !rows.is_empty() {
                self.pending = Some(row);
                break;
            }
            rows.push(row);
        }
        if rows.is_empty() {
            return Ok(None);
        }
        let bounds = rows.iter().map(|row| bounds_of(row, m.binding.outer)).collect::<Result<Vec<_>>>()?;
        let functions = rows.iter().map(|row| m.binding.prepared(row, m.prepared.as_ref())).collect::<Result<Vec<_>>>()?;
        let packed = Packed::new(&bounds)?;
        Ok(Some(Part { rows, bounds, functions, packed, _held: held }))
    }

    fn next_outer(&mut self, m: &Matching) -> Result<Option<Row>> {
        if let Some(row) = self.pending.take() {
            return Ok(Some(row));
        }
        self.outer.next().map(|record| record_row(&record?, &m.types)).transpose()
    }
}
