//! Scan the visible large input once, refine boundary-mask candidates,
//! and apply the remaining ON condition before emitting each pair.

use super::{Binding, Held, IndexJoin};
use crate::error::Result;
use crate::exec::{RowIter, Runner, Subqueries, broken, eval_cond, project::Columns};
use crate::functions::ScalarRef;
use crate::geometry::Geometry;
use crate::logical::Truth;
use crate::memory::Reservation;
use crate::spatial::{
    Bounds,
    packed::{Cursor, Packed},
};
use crate::storage::{Row, located};
use crate::txn::{Stop, checked};
use std::sync::Arc;

pub(super) fn open(join: IndexJoin, held: Held, (binding, packed): (Binding, Packed), columns: &[usize]) -> Result<RowIter> {
    let functions = held.rows.iter().map(|row| binding.prepared(row, join.budget.as_ref())).collect::<Result<Vec<_>>>()?;
    let stop = join.table.stop.clone();
    let located_read = join.table.located;
    let rows = Arc::clone(&join.table.storage).scan(join.table.tx, columns);
    let input = Box::new(checked(
        rows.map(move |row| {
            row.map(|(id, mut row)| {
                if located_read {
                    row.push(located(id));
                }
                row
            })
        }),
        stop.clone(),
    ));
    let scan = Scan {
        input,
        regions: held.rows,
        functions,
        binding,
        packed,
        runner: join.runner,
        output: join.output,
        current: None,
        stop: stop.clone(),
        _held: held.reservation,
    };
    Ok(Box::new(checked(scan, stop)))
}

struct Scan {
    input: RowIter,
    regions: Vec<Row>,
    functions: Vec<ScalarRef>,
    binding: Binding,
    packed: Packed,
    runner: Runner,
    output: Option<Columns>,
    current: Option<(Row, Bounds, Cursor)>,
    stop: Option<Arc<Stop>>,
    _held: Reservation,
}

impl Scan {
    fn step(&mut self) -> Result<Option<Row>> {
        loop {
            if let Some(stop) = &self.stop {
                stop.check()?;
            }
            if let Some((row, bounds, mut cursor)) = self.current.take() {
                let Some(at) = self.packed.next(bounds, &mut cursor)? else { continue };
                let kept = self.paired(&row, at);
                self.current = Some((row, bounds, cursor));
                if let Some(pair) = kept? {
                    return Ok(Some(pair));
                }
            } else {
                let Some(row) = self.input.next().transpose()? else { return Ok(None) };
                let value = row.get(self.binding.inner).ok_or_else(|| broken("a spatial scan column is missing"))?;
                let bounds = if value.is_null() { None } else { Geometry::from_value(value)?.bounds() };
                if let Some(bounds) = bounds {
                    self.current = Some((row, bounds, Cursor::default()));
                }
            }
        }
    }

    fn paired(&self, row: &Row, at: usize) -> Result<Option<Row>> {
        let outer = self.regions.get(at).ok_or_else(|| broken("a spatial candidate row is missing"))?;
        let function = self.functions.get(at).ok_or_else(|| broken("a spatial candidate function is missing"))?;
        joined((&self.binding, &self.runner, self.output.as_ref()), (outer, row), function)
    }
}

/// The pair of `outer` and `row`, as the join gives it, if the spatial
/// conjunct, called as `function`, and the rest of the condition are
/// true of it.
pub(super) fn joined(
    (binding, runner, output): (&Binding, &Runner, Option<&Columns>),
    (outer, row): (&Row, &Row),
    function: &ScalarRef,
) -> Result<Option<Row>> {
    if !binding.intersects(outer, row, function, runner.temp_lobs())? {
        return Ok(None);
    }
    if binding.rest.is_none()
        && let Some(output) = output
    {
        return output.pair(outer, row).map(Some);
    }
    let pair: Row = outer.iter().chain(row).cloned().collect();
    let kept = match &binding.rest {
        Some(cond) => eval_cond(cond, &pair, runner)? == Truth::True,
        None => true,
    };
    if !kept {
        return Ok(None);
    }
    match output {
        Some(output) => output.take(pair).map(Some),
        None => Ok(Some(pair)),
    }
}

impl Iterator for Scan {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Self::Item> {
        self.step().transpose()
    }
}
