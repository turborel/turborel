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
    current: Option<Current>,
    stop: Option<Arc<Stop>>,
    _held: Reservation,
}

/// The table row being paired with the regions its envelope meets.
struct Current {
    row: Row,
    bounds: Bounds,
    /// The point the row's geometry is, with its SRID, if its envelope is
    /// one point (`single_point`).
    point: Option<([f64; 2], i32)>,
    cursor: Cursor,
}

impl Scan {
    fn step(&mut self) -> Result<Option<Row>> {
        loop {
            if let Some(stop) = &self.stop {
                stop.check()?;
            }
            if let Some(mut current) = self.current.take() {
                let Some(at) = self.packed.next(current.bounds, &mut current.cursor)? else { continue };
                let kept = self.paired(&current.row, at, current.point);
                self.current = Some(current);
                if let Some(pair) = kept? {
                    return Ok(Some(pair));
                }
            } else {
                let Some(row) = self.input.next().transpose()? else { return Ok(None) };
                let value = row.get(self.binding.inner).ok_or_else(|| broken("a spatial scan column is missing"))?;
                let geometry = if value.is_null() { None } else { Some(Geometry::from_value(value)?) };
                if let Some((geometry, bounds)) = geometry.and_then(|geometry| geometry.bounds().map(|bounds| (geometry, bounds))) {
                    let point = single_point(bounds).map(|point| (point, geometry.srid()));
                    self.current = Some(Current { row, bounds, point, cursor: Cursor::default() });
                }
            }
        }
    }

    fn paired(&self, row: &Row, at: usize, point: Option<([f64; 2], i32)>) -> Result<Option<Row>> {
        let outer = self.regions.get(at).ok_or_else(|| broken("a spatial candidate row is missing"))?;
        let function = self.functions.get(at).ok_or_else(|| broken("a spatial candidate function is missing"))?;
        joined((&self.binding, &self.runner, self.output.as_ref()), (outer, row), function, point)
    }
}

/// The point a geometry is whose envelope `bounds` is one point, as an
/// index entry of it is decided (RULES.md D85): not the origin, which an
/// empty geometry projects to, left to the call.
fn single_point(bounds: Bounds) -> Option<[f64; 2]> {
    let ([x0, y0], [x1, y1]) = (bounds.lower(), bounds.upper());
    let single = x0.to_bits() == x1.to_bits() && y0.to_bits() == y1.to_bits();
    let origin = x0.abs() <= 0.0_f64 && y0.abs() <= 0.0_f64;
    (single && !origin).then_some([x0, y0])
}

/// The pair of `outer` and `row`, as the join gives it, if the spatial
/// conjunct, called as `function`, or decided of `point` if `row`'s
/// geometry is one (`Binding::point_intersects`), and the rest of the
/// condition are true of it.
pub(super) fn joined(
    (binding, runner, output): (&Binding, &Runner, Option<&Columns>),
    (outer, row): (&Row, &Row),
    function: &ScalarRef,
    point: Option<([f64; 2], i32)>,
) -> Result<Option<Row>> {
    let intersects = match point.and_then(|point| binding.point_intersects(outer, function, point)) {
        Some(decided) => decided?,
        None => binding.intersects(outer, row, function, runner.temp_lobs())?,
    };
    if !intersects {
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
