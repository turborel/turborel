//! [ISO/IEC 9075-2:2023 §6.11 GR] Nested window functions, run: the
//! argument of a window function's set function that holds them, over
//! each row's frame. Each row's places hold the input's columns the
//! argument reads, of its own row or another's, in place of the argument.
//! A context row of the input's width, holding those columns of the
//! frame's row R2 and after them each nested function's value, inner
//! first, is what the argument and each default are evaluated over
//! unchanged; a value expression is evaluated over the columns of the row
//! its marker and offset give instead [GR4e ii].
//!
//! An argument of no marker of the current row R1 or of its frame
//! (`Marker::of_current`) is of R2 and the partition alone, so it is
//! computed once for each row, into a list of its own, and the set
//! function run over that list as over any argument (`aggregate.rs`). Any
//! other is computed afresh for each row of each frame and fed to a new
//! accumulator, the value of the frame of the row before reused only where
//! no marker is CURRENT_ROW. An empty frame evaluates nothing [NOTE 170].

use super::LIST_SEGMENTS;
use super::aggregate::{Framed, fresh, take};
use super::frame::{Bounds, EMPTY, Partition, Ranges};
use super::reader::{Placed, Reader};
use crate::error::Result;
use crate::exec::group::admitted;
use crate::exec::work::{Bag, Work};
use crate::exec::{HeldRows, Runner, broken, eval};
use crate::logical::{AggCall, Cond, Expr, Marker, Nested, WindowCall, WindowKind, expr_columns};
use crate::spill::ListSpace;
use crate::storage::Row;
use crate::value::{Decimal, Value};

/// The frame a precomputed argument is found of: none, as no marker of
/// such an argument reads it.
const NO_FRAME: Ranges = [EMPTY, EMPTY, EMPTY];

/// What a call holding nested window functions reads of each row and how
/// far its markers move, found as the first row is read.
#[derive(Debug)]
pub(super) struct Shape {
    /// The input's columns its expressions read, in order.
    used: Vec<usize>,
    /// The input's width: where the nested functions' values begin in a
    /// context row.
    width: usize,
    /// Of each nested function, the rows its `<row marker delta>` moves
    /// its marker by, 0 if none is written; none if its offset is null.
    deltas: Vec<Option<i128>>,
}

impl Shape {
    /// That of `call` over rows of `width` columns, its offsets evaluated
    /// once [§7.15 GR5b]; none if it holds no nested window function.
    ///
    /// # Errors
    ///
    /// An offset's evaluation's.
    pub(super) fn of(call: &WindowCall, width: usize, runner: &Runner) -> Result<Option<Shape>> {
        if call.nested.is_empty() {
            return Ok(None);
        }
        let mut used = Vec::new();
        call.exprs().for_each(|e| expr_columns(e, &mut used));
        used.retain(|&c| c < width);
        used.sort_unstable();
        used.dedup();
        let deltas = call.nested.iter().map(|n| delta(n, runner)).collect::<Result<_>>()?;
        Ok(Some(Shape { used, width, deltas }))
    }

    /// How many of a row's `Placed` values its call takes.
    pub(super) fn taken(&self) -> usize {
        self.used.len()
    }

    /// Adds to `values` the columns of `row` its call reads.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `row` has not one of them.
    pub(super) fn take_from(&self, row: &Row, values: &mut Row) -> Result<()> {
        for &c in &self.used {
            values.push(row.get(c).cloned().ok_or_else(|| broken("a window's row has not a column its argument reads"))?);
        }
        Ok(())
    }
}

/// \[GR4d\] The rows the offset of `n` moves its marker by: less if after a
/// minus sign; none if null.
fn delta(n: &Nested, runner: &Runner) -> Result<Option<i128>> {
    let Some((minus, offset)) = &n.delta else { return Ok(Some(0)) };
    let by = eval(offset, &Vec::new(), runner)?;
    if by.is_null() {
        return Ok(None);
    }
    if let Value::Exact(d) = &by
        && d.scale == 0
    {
        return Ok(Some(if *minus { d.units.saturating_neg() } else { d.units }));
    }
    Err(broken("a <row marker offset> is not an exact number of scale 0"))
}

/// Whether an argument holding `nested` is of the frame's row and its
/// partition alone: no marker is of the current row or of its frame.
fn alone(nested: &[Nested]) -> bool {
    !nested.iter().any(|n| n.marker.of_current())
}

/// \[DESIGN\] How many more segments of a partition's places a call
/// holding `nested` keeps at once than a set function: one for each
/// VALUE_OF's row; and of an argument computed once, its list's in memory
/// and its reader's, as its reader of each row is let go by then.
pub(super) fn readers(nested: &[Nested]) -> usize {
    let values = nested.iter().filter(|n| n.value.is_some()).count();
    values.saturating_add(if alone(nested) { LIST_SEGMENTS } else { 0 })
}

/// The rows the nested window functions of a call are evaluated over.
struct Context<'a> {
    nested: &'a [Nested],
    shape: &'a Shape,
    /// Where the call's values begin among a row's `Placed` values.
    slot: usize,
    /// R2's columns, then each nested function's value.
    row: Row,
    /// The reader of R2's row.
    walker: Reader,
    /// Of each nested function, the reader of the row its marker gives.
    readers: Vec<Reader>,
}

impl<'a> Context<'a> {
    /// That of `nested`, of `shape`, the call's values from `slot` on.
    fn new(nested: &'a [Nested], shape: &'a Shape, slot: usize) -> Context<'a> {
        let row = vec![Value::Null; shape.width.saturating_add(nested.len())];
        Context { nested, shape, slot, row, walker: Reader::default(), readers: nested.iter().map(|_| Reader::default()).collect() }
    }

    /// \[GR1\] The argument `arg` at R2, the `r2`th row of `part`, of the
    /// frame `frame` of R1, the `r1`th: each nested function's value
    /// found, inner first, then the argument.
    fn argument(&mut self, arg: &Expr, (r1, r2, frame): (usize, usize, &Ranges), part: &mut Partition, runner: &Runner) -> Result<Value> {
        let own = columns(self.walker.get(&mut part.keys, r2)?, self.slot, self.shape.taken())?.to_vec();
        put(&mut self.row, &self.shape.used, &own)?;
        for (k, n) in self.nested.iter().enumerate() {
            let number = row_number(n.marker, (r1, r2), frame, part.len)?;
            let value = match &n.value {
                None => count(number),
                Some((value, default)) => {
                    let delta = self.shape.deltas.get(k).copied().ok_or_else(|| broken("a nested window function has no offset"))?;
                    match place(number, delta, part.len) {
                        None => eval(default, &self.row, runner)?,
                        Some(m) => self.value_at((k, m), (value, &own), part, runner)?,
                    }
                }
            };
            let at = self.shape.width.saturating_add(k);
            *self.row.get_mut(at).ok_or_else(|| broken("a context row has no place of a nested window function"))? = value;
        }
        eval(arg, &self.row, runner)
    }

    /// [ISO/IEC 9075-2:2023 §10.9 GR4] Whether the set function of the
    /// filter `filter` takes R2, the `r2`th row of `part`: the filter, of
    /// R2's columns, true of it, or none written.
    fn admits(&mut self, filter: Option<&Cond>, r2: usize, part: &mut Partition, runner: &Runner) -> Result<bool> {
        let Some(c) = filter else { return Ok(true) };
        let own = columns(self.walker.get(&mut part.keys, r2)?, self.slot, self.shape.taken())?.to_vec();
        put(&mut self.row, &self.shape.used, &own)?;
        admitted(Some(c), &self.row, runner)
    }

    /// [GR4e ii] The value expression `value` of the `k`th nested function
    /// evaluated at the `m`th row of `part`, its columns put in the context
    /// row for it, then R2's, `own`, put back.
    fn value_at(
        &mut self,
        (k, m): (usize, usize),
        (value, own): (&Expr, &[Value]),
        part: &mut Partition,
        runner: &Runner,
    ) -> Result<Value> {
        let reader = self.readers.get_mut(k).ok_or_else(|| broken("a nested window function has no reader"))?;
        put(&mut self.row, &self.shape.used, columns(reader.get(&mut part.keys, m)?, self.slot, self.shape.taken())?)?;
        let v = eval(value, &self.row, runner);
        put(&mut self.row, &self.shape.used, own)?;
        v
    }
}

/// The `taken` values of `placed` from `slot` on: the columns of its row
/// a call reads.
fn columns(placed: &Placed, slot: usize, taken: usize) -> Result<&[Value]> {
    placed.values.get(slot..slot.saturating_add(taken)).ok_or_else(|| broken("a window's row has not the columns its argument reads"))
}

/// Puts `values` in `row` at `used`, each at its column.
fn put(row: &mut Row, used: &[usize], values: &[Value]) -> Result<()> {
    for (&c, v) in used.iter().zip(values) {
        *row.get_mut(c).ok_or_else(|| broken("a context row has no place of a column"))? = v.clone();
    }
    Ok(())
}

/// [GR2, GR3b] The window partition row number of `marker`'s row, from 1,
/// R1 the `r1`th row and R2 the `r2`th of a partition of `len` rows, in
/// the frame `frame` of R1: its first and last rows those of its first
/// and last ranges not empty, as exclusion leaves them.
fn row_number(marker: Marker, (r1, r2): (usize, usize), frame: &Ranges, len: usize) -> Result<usize> {
    let empty = || broken("a nested window function evaluated in an empty frame");
    let mut ranges = frame.iter().filter(|r| r.0 < r.1);
    Ok(match marker {
        Marker::BeginPartition => 1,
        Marker::EndPartition => len,
        Marker::CurrentRow => r1.saturating_add(1),
        Marker::FrameRow => r2.saturating_add(1),
        Marker::BeginFrame => ranges.next().ok_or_else(empty)?.0.saturating_add(1),
        Marker::EndFrame => ranges.next_back().ok_or_else(empty)?.1,
    })
}

/// [GR4d, GR4e i] The place, from 0, of row M, the row `number` moved by
/// `delta`, in a partition of `len` rows: none if the offset is null (no
/// `delta`), M is below 1 or past `len`.
fn place(number: usize, delta: Option<i128>, len: usize) -> Option<usize> {
    let m = i128::try_from(number).ok()?.checked_add(delta?)?;
    usize::try_from(m.checked_sub(1)?).ok().filter(|&p| p < len)
}

/// \[SR2\] The row number `n`, of COUNT's type (IV085).
fn count(n: usize) -> Value {
    Value::Exact(Decimal::new(i128::try_from(n).unwrap_or(i128::MAX), 0))
}

/// A set function holding nested window functions over each row's frame
/// in a partition.
pub(super) struct Nest<'a> {
    call: &'a AggCall,
    way: Way<'a>,
}

/// How its frames are computed.
enum Way<'a> {
    /// Its argument computed once for each row, into `own`, and the set
    /// function run over that.
    Alone { framed: Framed<'a>, own: Partition },
    /// Its argument computed for each row of each frame, by `context`; a
    /// DISTINCT one's values kept in a bag of `work`; the last frame and
    /// its value and nulls, reused if `cached`.
    Walked { context: Context<'a>, arg: &'a Expr, work: Work, cached: bool, last: Option<(Ranges, Value, bool)> },
}

impl<'a> Nest<'a> {
    /// The set function of `call`, of `shape`, over the frames `bounds`
    /// finds of `part`, its values from `slot` on; an argument computed
    /// once held in `keyed` and given a tree of states of at most `tree`
    /// bytes, a DISTINCT one's values kept in `work`.
    ///
    /// # Errors
    ///
    /// The spill area's; the argument's evaluation's; the set function's.
    pub(super) fn new(
        (call, shape): (&'a WindowCall, &'a Shape),
        (bounds, slot): (&Bounds, usize),
        part: &mut Partition,
        (keyed, tree, work): (&ListSpace, usize, &Work),
        runner: &Runner,
    ) -> Result<Nest<'a>> {
        let WindowKind::Aggregate(agg) = &call.kind else { return Err(broken("nested window functions not in a set function")) };
        let arg = agg.arg.as_ref().ok_or_else(|| broken("nested window functions in COUNT(*)"))?;
        let mut context = Context::new(&call.nested, shape, slot);
        let way = if alone(&call.nested) {
            let mut list = HeldRows::new(keyed.clone());
            for row in 0..part.len {
                // Of a row a filter does not take, no argument is computed,
                // and that it is not taken held after it, as `Framed` reads.
                let taken = context.admits(agg.filter.as_ref(), row, part, runner)?;
                let mut values = vec![if taken { context.argument(arg, (row, row, &NO_FRAME), part, runner)? } else { Value::Null }];
                if agg.filter.is_some() {
                    values.push(Value::Bool(taken));
                }
                list.push(Placed { group: 0, values })?;
            }
            let mut own = Partition { keys: list, len: part.len, nulls: part.nulls };
            Way::Alone { framed: Framed::new(agg, (bounds, 0), &mut own, tree, work)?, own }
        } else if agg.distinct && !bounds.grows() {
            return Err(broken("a DISTINCT set function over a frame that moves"));
        } else {
            let cached = !call.nested.iter().any(|n| n.marker == Marker::CurrentRow);
            Way::Walked { context, arg, work: work.another(), cached, last: None }
        };
        Ok(Nest { call: agg, way })
    }

    /// Its value over `frame`, the frame of the `row`th row of `part`, and
    /// whether it left out a null.
    ///
    /// # Errors
    ///
    /// The spill area's; the argument's evaluation's; the set function's.
    pub(super) fn value(&mut self, frame: Ranges, row: usize, part: &mut Partition, runner: &Runner) -> Result<(Value, bool)> {
        let (context, arg, work, cached, last) = match &mut self.way {
            Way::Alone { framed, own } => return framed.value(frame, own),
            Way::Walked { context, arg, work, cached, last } => (context, *arg, work, *cached, last),
        };
        if cached
            && let Some((before, value, nulls)) = last.as_ref()
            && *before == frame
        {
            return Ok((value.clone(), *nulls));
        }
        let (mut acc, mut nulls) = (fresh(self.call), false);
        let mut seen = self.call.distinct.then(|| Bag::new(work.another()));
        for &(from, to) in &frame {
            for r2 in from..to {
                if !context.admits(self.call.filter.as_ref(), r2, part, runner)? {
                    continue;
                }
                let v = context.argument(arg, (row, r2, &frame), part, runner)?;
                take(acc.as_mut(), Some(&v), (&mut nulls, seen.as_mut()))?;
            }
        }
        let value = acc.finish()?;
        *last = Some((frame, value.clone(), nulls));
        Ok((value, nulls))
    }
}

// Unit tests: nested window functions against their General Rules by
// hand, over random partitions and frames (RULES.md R-TEST-1, R-TEST-5).
#[cfg(test)]
mod tests;
