//! [ISO/IEC 9075-2:2023 §7.15, §6.10] Window functions, run: the rows of
//! the input, in order of their window partitioning and ordering (the
//! planner sorts them, unless they come so), read one partition at a time
//! [GR3, GR4]. Each partition's rows are held, as are, of each, its peer
//! group's number, its range key and its calls' arguments, up to a part of
//! the work area each and spilled past it (RULES.md D38). Then each row's
//! value of each call is found, its frame's bounds from pointers that only
//! move forward (`frame.rs`), and the rows given with their values, before
//! the next partition is read. A set function holding nested window
//! functions [§6.11] holds of each row the columns its argument reads and
//! computes its argument with the frames (`nested.rs`). An inverse
//! distribution function, whose frame is its whole partition, sorts the
//! partition's values once (`distribution.rs`).
//!
//! The rows a window function compares are those of its partition in its
//! ordering: no partition's rows depend on another's. Peers are given in the
//! order the sort gives them, one for every window of one partitioning and
//! ordering, as one node computes them all (US035).
//!
//! Each row is taken only while the statement is not cancelled (RULES.md
//! D44).

mod aggregate;
mod distribution;
mod frame;
mod nested;
mod reader;
mod value;

use std::cmp::Ordering;
use std::sync::Arc;

use self::aggregate::Framed;
use self::distribution::{DISTRIBUTION_READERS, Distributed};
use self::frame::{Bounds, Current, Partition};
use self::nested::{Nest, Shape};
use self::reader::{Placed, Reader};
use self::value::{Count, NotNull, Picked, RANGE_ENDS, Sequence, counted, tile};
use super::group::admitted;
use super::work::Work;
use super::{HeldRows, RowIter, Runner, broken, column, eval, execute, failed, warn};
use crate::db::State;
use crate::error::{Result, SqlState};
use crate::logical::{AggCall, SortKey, Window, WindowCall, WindowKind};
use crate::physical::Plan;
use crate::spill::ListSpace;
use crate::storage::Row;
use crate::txn::{TxId, checked};
use crate::value::{Decimal, Value};

/// How many parts of the work area a partition's rows, their values, the
/// places' values its frames are found by, and the trees of states each
/// take at most.
const PARTS: usize = 4;

/// The readers of a partition's places its peers are found by: of the
/// current row, and of the rows after it (`Peers`).
const PEER_READERS: usize = 2;

/// The readers of a frame's places: of its start and of its end (`Bounds`).
const BOUND_READERS: usize = 2;

/// The readers of LEAD or LAG: of the current row and of the row whose
/// extent it takes (`Picked`). Under IGNORE NULLS, a third reads the row
/// after it or at it, for the count of arguments not null before.
const OFFSET_READERS: usize = 2;

/// The segments of a list of arguments not null: the one it holds in
/// memory, and its reader's.
const LIST_SEGMENTS: usize = 2;

/// The values LEAD or LAG takes of each row: its extent and its default.
const OFFSET_VALUES: usize = 2;

/// The rows of `plan`, window functions over its input.
#[inline(never)]
pub(super) fn windowed(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    let Plan::Window { window, .. } = plan else { return failed(broken("a window is not one")) };
    let (w, input) = window.split();
    let partition = w.partition.iter().map(|&c| SortKey::ascending(c)).collect();
    let rows = execute(input, db, tx);
    let (db_held, runner) = (Arc::clone(db), Runner::new(db, tx));
    let (slots, shapes) = (Vec::new(), Vec::new());
    let given = Windowed { w, partition, slots, shapes, rows, runner, db: db_held, tx, bounds: None, next: None, given: None, done: false };
    Box::new(checked(given, db.tx.stop_of(tx)))
}

/// The rows of one partition, given with their calls' values.
type Given = Box<dyn Iterator<Item = Result<Row>> + Send>;

/// Window functions over rows in order, a partition at a time.
struct Windowed {
    w: Window<()>,
    /// The partitioning columns, as keys a row's are compared by.
    partition: Vec<SortKey>,
    /// Where each call's values begin among a row's `Placed` values,
    /// found as the first row is read.
    slots: Vec<usize>,
    /// Of each call holding nested window functions, what it reads of each
    /// row, found as the first row is read.
    shapes: Vec<Option<Shape>>,
    rows: RowIter,
    runner: Runner,
    db: Arc<State>,
    tx: TxId,
    /// Each call's frame bounds and count, their offsets and counts
    /// evaluated as the first row is read [§7.15 GR5b, §6.10 GR1a i, GR1d
    /// i].
    bounds: Option<Vec<(Bounds, Count)>>,
    /// The first row of the next partition, once read.
    next: Option<Row>,
    /// The rows of the partition being given.
    given: Option<Given>,
    /// Whether the input is read to its end, or failed.
    done: bool,
}

impl Iterator for Windowed {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        loop {
            if let Some(given) = &mut self.given {
                if let Some(row) = given.next() {
                    return Some(row);
                }
                self.given = None;
            }
            if self.done {
                return None;
            }
            match self.partition() {
                Ok(Some(given)) => self.given = Some(given),
                Ok(None) => self.done = true,
                Err(e) => {
                    self.done = true;
                    return Some(Err(e));
                }
            }
        }
    }
}

/// The bytes of a part of the work area, of `limit` bytes.
fn part_of(limit: usize) -> usize {
    limit.checked_div(PARTS).unwrap_or_default().max(1)
}

impl Windowed {
    /// The rows of the next partition with their calls' values; none past
    /// the last.
    fn partition(&mut self) -> Result<Option<Given>> {
        let first = match self.next.take() {
            Some(row) => row,
            None => match self.rows.next() {
                Some(row) => row?,
                None => return Ok(None),
            },
        };
        if self.bounds.is_none() {
            // The input's rows are all of its width, the first's.
            let shaped = |c: &WindowCall| Shape::of(c, first.len(), &self.runner);
            self.shapes = self.w.calls.iter().map(shaped).collect::<Result<_>>()?;
            self.slots = slots(&self.w.calls, &self.shapes);
            let order = self.w.order.first().map(SortKey::order);
            let prepared = |c: &WindowCall| Ok((Bounds::of(&c.frame, order, &self.runner)?, Count::of(&c.kind, &self.runner)?));
            self.bounds = Some(self.w.calls.iter().map(prepared).collect::<Result<_>>()?);
        }
        let work = Work::of(&self.db, self.tx);
        let space = ListSpace { limit: part_of(work.limit()), ..self.db.list_space() };
        // [DESIGN] Each reader of the places' values keeps a segment of
        // them, and each list of values not null holds one in memory and
        // its reader another (`readers`).
        let readers = self.w.calls.iter().map(readers).fold(PEER_READERS, usize::saturating_add);
        let keyed = ListSpace { limit: space.limit.checked_div(readers).unwrap_or_default().max(1), ..space.clone() };
        let (rows, mut part, lists) = self.read(first, (space.clone(), keyed.clone()))?;
        let tree = space.limit.checked_div(self.w.calls.len()).unwrap_or_default();
        let values = self.values(&mut part, lists, (space, keyed, tree), &work)?;
        let given = rows.into_iter().zip(values).map(|(row, values)| {
            let mut row = row?;
            row.extend(values?);
            Ok(row)
        });
        Ok(Some(Box::new(given)))
    }

    /// The rows of the partition `first` begins, held in `space`, and of
    /// each, what its frames are found by, held in `keyed`; the first row
    /// of the next partition kept.
    fn read(&mut self, first: Row, (space, keyed): (ListSpace, ListSpace)) -> Result<(HeldRows<Row>, Partition, Vec<Option<NotNull>>)> {
        let parted = self.partition.iter().map(|k| column(&first, k.column).cloned()).collect::<Result<Vec<_>>>()?;
        let mut lists: Vec<Option<NotNull>> =
            self.w.calls.iter().map(|c| ignores_nulls(&c.kind).then(|| NotNull { values: HeldRows::new(keyed.clone()), len: 0 })).collect();
        let (mut rows, mut keys) = (HeldRows::new(space), HeldRows::new(keyed));
        let (mut group, mut last, mut nulls) = (0_u64, None::<Vec<Value>>, None::<(usize, usize)>);
        let mut row = first;
        loop {
            let order = self.w.order.iter().map(|k| column(&row, k.column).cloned()).collect::<Result<Vec<_>>>()?;
            if last.as_ref().is_some_and(|last| !alike(&self.w.order, last, &row)) {
                group = group.saturating_add(1);
            }
            let key = order.first().cloned().unwrap_or(Value::Null);
            if key.is_null() && !order.is_empty() {
                let place = rows.len();
                nulls = Some((nulls.map_or(place, |n| n.0), place.saturating_add(1)));
            }
            keys.push(Placed { group, values: self.arguments(key, &row, &mut lists)? })?;
            rows.push(row)?;
            last = Some(order);
            row = match self.rows.next() {
                None => break,
                Some(next) => next?,
            };
            if !alike(&self.partition, &parted, &row) {
                self.next = Some(row);
                break;
            }
        }
        let len = rows.len();
        Ok((rows, Partition { keys, len, nulls: nulls.unwrap_or((len, len)) }, lists))
    }

    /// The values a row's frames are found by: its range key `key`, then
    /// of each call at its slot, of `row`: an aggregate's argument, null of
    /// COUNT(*) [§10.9 GR4], or the columns it reads if it holds nested
    /// window functions; LEAD or LAG's extent and default; a value
    /// function's argument; and under IGNORE NULLS, how many arguments not
    /// null are before it, the row's added to its call's list of `lists`
    /// if not null.
    fn arguments(&self, key: Value, row: &Row, lists: &mut [Option<NotNull>]) -> Result<Row> {
        let mut values = Vec::with_capacity(self.slots.last().copied().unwrap_or(1).saturating_add(self.w.calls.len()));
        values.push(key);
        for ((call, list), shape) in self.w.calls.iter().zip(lists.iter_mut()).zip(&self.shapes) {
            if let Some(shape) = shape {
                shape.take_from(row, &mut values)?;
                continue;
            }
            let (arg, default, fraction) = match &call.kind {
                // [ISO/IEC 9075-2:2023 §10.9 GR4] of a row its filter is not
                // true of, no argument computed, and that it is not taken;
                // [GR10a] of an inverse distribution function, the row's
                // fraction.
                WindowKind::Aggregate(a) => {
                    let taken = admitted(a.filter.as_ref(), row, &self.runner)?;
                    let arg = match (&a.arg, taken) {
                        (Some(e), true) => eval(e, row, &self.runner)?,
                        (Some(_), false) | (None, _) => Value::Null,
                    };
                    let fraction = a.distribution.as_ref().map(|d| eval(&d.fraction, row, &self.runner)).transpose()?;
                    (arg, a.filter.as_ref().map(|_| Value::Bool(taken)), fraction)
                }
                WindowKind::Offset(o) => (eval(&o.extent, row, &self.runner)?, Some(eval(&o.default, row, &self.runner)?), None),
                WindowKind::Value(v) => (eval(&v.arg, row, &self.runner)?, None, None),
                WindowKind::RowNumber
                | WindowKind::Rank
                | WindowKind::DenseRank
                | WindowKind::PercentRank
                | WindowKind::CumeDist
                | WindowKind::Ntile(_) => continue,
            };
            values.push(arg.clone());
            values.extend(default);
            values.extend(fraction);
            if let Some(list) = list {
                values.push(counted(list.len));
                if !arg.is_null() {
                    list.values.push(Placed { group: 0, values: vec![arg] })?;
                    list.len = list.len.saturating_add(1);
                }
            }
        }
        Ok(values)
    }

    /// Of each row of `part`, its calls' values, held in `space`: under
    /// IGNORE NULLS of each of `lists`, a tree of states each taking at
    /// most `tree` bytes, a DISTINCT set function's values kept in `work`,
    /// an argument of nested window functions computed once held in
    /// `keyed`.
    fn values(
        &mut self,
        part: &mut Partition,
        lists: Vec<Option<NotNull>>,
        (space, keyed, tree): (ListSpace, ListSpace, usize),
        work: &Work,
    ) -> Result<HeldRows<Row>> {
        let Some(bounds) = self.bounds.as_mut() else { return Err(broken("a window's frames were not bound")) };
        for (b, _) in bounds.iter_mut() {
            b.reset();
        }
        let mut computed = Vec::with_capacity(self.w.calls.len());
        let calls = self.w.calls.iter().zip(&self.shapes).zip(bounds.iter());
        for (((call, shape), (b, count)), (&slot, list)) in calls.zip(self.slots.iter().zip(lists)) {
            computed.push(match (shape, distributed(&call.kind)) {
                (Some(shape), _) => Computed::Nested(Nest::new((call, shape), (b, slot), part, (&keyed, tree, work), &self.runner)?),
                (None, Some(a)) => Computed::Distributed(Distributed::new(a, slot, part, (&keyed, work))?),
                (None, None) => Computed::new(&call.kind, (b, *count), (slot, list), part, (tree, work))?,
            });
        }
        let (mut out, mut peers) = (HeldRows::new(space), Peers::default());
        for row in 0..part.len {
            let current = current(row, part, &mut peers)?;
            let mut values = Vec::with_capacity(computed.len());
            for (c, (b, _)) in computed.iter_mut().zip(bounds.iter_mut()) {
                values.push(c.value((b, &current), part, &self.runner)?);
            }
            out.push(values)?;
        }
        Ok(out)
    }
}

/// How a call's value is computed, row after row of a partition.
enum Computed<'a> {
    /// A rank function's or NTILE's, of its count, from the row's place
    /// and peers.
    Ranked(&'a WindowKind, Count),
    /// A set function's, over each frame.
    Framed(Framed<'a>),
    /// LEAD or LAG's.
    Offset(Picked<'a>),
    /// A value function's, over each frame.
    Valued(Picked<'a>),
    /// A set function's holding nested window functions, over each frame.
    Nested(Nest<'a>),
    /// An inverse distribution function's, over the whole partition.
    Distributed(Distributed<'a>),
}

impl<'a> Computed<'a> {
    /// That of `kind`, of the frames `bounds` finds and `count`, its values
    /// from `slot` on, its arguments not null `list` if IGNORE NULLS, of
    /// `part`; a set function's tree of states of at most `tree` bytes, a
    /// DISTINCT one's values kept in `work`.
    fn new(
        kind: &'a WindowKind,
        (bounds, count): (&Bounds, Count),
        (slot, list): (usize, Option<NotNull>),
        part: &mut Partition,
        (tree, work): (usize, &Work),
    ) -> Result<Computed<'a>> {
        let sequence = || list.map_or(Sequence::Each, Sequence::NotNull);
        Ok(match kind {
            WindowKind::Aggregate(a) => Computed::Framed(Framed::new(a, (bounds, slot), part, tree, work)?),
            WindowKind::Offset(_) => Computed::Offset(Picked::new(kind, slot, count, sequence())),
            WindowKind::Value(_) => Computed::Valued(Picked::new(kind, slot, count, sequence())),
            WindowKind::RowNumber
            | WindowKind::Rank
            | WindowKind::DenseRank
            | WindowKind::PercentRank
            | WindowKind::CumeDist
            | WindowKind::Ntile(_) => Computed::Ranked(kind, count),
        })
    }

    /// Its value of `current`, a row of `part`, its frame of `bounds`.
    fn value(&mut self, (bounds, current): (&mut Bounds, &Current), part: &mut Partition, runner: &Runner) -> Result<Value> {
        match self {
            Computed::Ranked(kind, count) => rank(kind, current, part.len, *count),
            Computed::Framed(f) => {
                let frame = bounds.frame(current, part)?;
                Ok(noted(f.value(frame, part)?, runner))
            }
            Computed::Nested(n) => {
                let frame = bounds.frame(current, part)?;
                Ok(noted(n.value(frame, current.row, part, runner)?, runner))
            }
            Computed::Distributed(d) => Ok(noted(d.value(current.row, part)?, runner)),
            Computed::Offset(p) => p.offset(current.row, part),
            Computed::Valued(p) => {
                let frame = bounds.frame(current, part)?;
                p.framed(frame, part)
            }
        }
    }
}

/// How many of a row's `Placed` values the call `kind` takes: an
/// aggregate its argument, if filtered whether it takes the row, and if an
/// inverse distribution function its fraction; LEAD or LAG its extent and
/// default; a value function its argument; and under IGNORE NULLS, one
/// more, the count of arguments not null before the row.
fn width(kind: &WindowKind) -> usize {
    let (taken, counted) = match kind {
        WindowKind::Aggregate(a) => {
            (1_usize.saturating_add(usize::from(a.filter.is_some())).saturating_add(usize::from(a.distribution.is_some())), false)
        }
        WindowKind::Offset(o) => (OFFSET_VALUES, o.ignore_nulls),
        WindowKind::Value(v) => (1_usize, v.ignore_nulls),
        WindowKind::RowNumber
        | WindowKind::Rank
        | WindowKind::DenseRank
        | WindowKind::PercentRank
        | WindowKind::CumeDist
        | WindowKind::Ntile(_) => (0_usize, false),
    };
    taken.saturating_add(usize::from(counted))
}

/// Where each of `calls`' values begin among a row's `Placed` values,
/// after its range key, of each its `shapes` if it holds nested window
/// functions.
fn slots(calls: &[WindowCall], shapes: &[Option<Shape>]) -> Vec<usize> {
    let mut at = 1_usize;
    calls
        .iter()
        .zip(shapes)
        .map(|(c, shape)| {
            let slot = at;
            at = at.saturating_add(shape.as_ref().map_or_else(|| width(&c.kind), Shape::taken));
            slot
        })
        .collect()
}

/// The call `kind`, if an inverse distribution function.
fn distributed(kind: &WindowKind) -> Option<&AggCall> {
    match kind {
        WindowKind::Aggregate(a) => a.distribution.as_ref().map(|_| a),
        WindowKind::RowNumber
        | WindowKind::Rank
        | WindowKind::DenseRank
        | WindowKind::PercentRank
        | WindowKind::CumeDist
        | WindowKind::Ntile(_)
        | WindowKind::Offset(_)
        | WindowKind::Value(_) => None,
    }
}

/// Whether the call `kind` counts only the rows whose argument is not
/// null: under IGNORE NULLS.
fn ignores_nulls(kind: &WindowKind) -> bool {
    match kind {
        WindowKind::Offset(o) => o.ignore_nulls,
        WindowKind::Value(v) => v.ignore_nulls,
        WindowKind::RowNumber
        | WindowKind::Rank
        | WindowKind::DenseRank
        | WindowKind::PercentRank
        | WindowKind::CumeDist
        | WindowKind::Aggregate(_)
        | WindowKind::Ntile(_) => false,
    }
}

/// \[DESIGN\] How many segments of a partition's places `call` keeps at
/// once: of its frame's two bounds; an aggregate's or a value function's
/// reader of its rows, and an aggregate's more of nested window functions
/// (`nested::readers`); an inverse distribution function's of its rows'
/// fractions and its sorted values (`distribution.rs`); LEAD or LAG's of the current row and the row
/// taken; under IGNORE NULLS, LEAD or LAG's of the row ahead, a value
/// function's of the ends of its frame's ranges, and the list of values
/// not null and its reader.
fn readers(call: &WindowCall) -> usize {
    let kind = &call.kind;
    let (listed, ends, ahead) = if ignores_nulls(kind) { (LIST_SEGMENTS, RANGE_ENDS, 1) } else { (0, 0, 0) };
    match kind {
        WindowKind::Aggregate(a) => match a.distribution {
            Some(_) => DISTRIBUTION_READERS,
            None => BOUND_READERS.saturating_add(1).saturating_add(nested::readers(&call.nested)),
        },
        WindowKind::Value(_) => BOUND_READERS.saturating_add(1).saturating_add(listed).saturating_add(ends),
        WindowKind::Offset(_) => OFFSET_READERS.saturating_add(listed).saturating_add(ahead),
        WindowKind::RowNumber
        | WindowKind::Rank
        | WindowKind::DenseRank
        | WindowKind::PercentRank
        | WindowKind::CumeDist
        | WindowKind::Ntile(_) => 0,
    }
}

/// The value of a set function, `value`, warned of if it left out a
/// null (`nulls`).
fn noted((value, nulls): (Value, bool), runner: &Runner) -> Value {
    // [ISO/IEC 9075:1992 §6.5 GR1b] a null left out warns.
    if nulls {
        warn(runner, SqlState::NULL_VALUE_ELIMINATED);
    }
    value
}

/// Whether `row`'s values of `keys` are those of `values`: of one
/// partition, or peers [§7.15 GR3, §10.10 GR1e ii 1].
fn alike(keys: &[SortKey], values: &[Value], row: &Row) -> bool {
    keys.iter().zip(values).all(|(k, v)| row.get(k.column).is_some_and(|x| k.order().cmp(v, x) == Ordering::Equal))
}

/// The peers of the rows of a partition, found row after row: a reader of
/// the current row, one of the rows after it, and the places of its peers.
#[derive(Default)]
struct Peers {
    own: Reader,
    ahead: Reader,
    from: usize,
    to: usize,
}

/// The `row`th row of `part`, the rows before it of `peers`.
fn current(row: usize, part: &mut Partition, peers: &mut Peers) -> Result<Current> {
    let placed = peers.own.get(&mut part.keys, row)?;
    let (group, key) = (placed.group, placed.values.first().cloned().unwrap_or(Value::Null));
    if row >= peers.to {
        (peers.from, peers.to) = (row, row.saturating_add(1));
        while peers.to < part.len && peers.ahead.get(&mut part.keys, peers.to)?.group == group {
            peers.to = peers.to.saturating_add(1);
        }
    }
    Ok(Current { row, group, peers: (peers.from, peers.to), key })
}

/// [ISO/IEC 9075-2:2023 §9.23 SR5, §6.10 GR1a] The value of the rank
/// function or NTILE `kind`, of `count`, of `current`, a row of a
/// partition of `len` rows: NTILE the tile of the row, null of a count
/// null; ROW_NUMBER its
/// place, RANK 1 plus the rows before its peers, DENSE_RANK 1 plus the
/// peer groups before its own, each of COUNT's type (IV166); PERCENT_RANK
/// (RANK - 1) / (`len` - 1), or 0 of one row, and CUME_DIST the rows up to
/// its last peer over `len`, approximate (IV082, IV083).
fn rank(kind: &WindowKind, current: &Current, len: usize, tiles: Count) -> Result<Value> {
    let count = |n: usize| Value::Exact(Decimal::new(i128::try_from(n).unwrap_or(i128::MAX).saturating_add(1), 0));
    Ok(match kind {
        WindowKind::RowNumber => count(current.row),
        WindowKind::Rank => count(current.peers.0),
        WindowKind::DenseRank => Value::Exact(Decimal::new(i128::from(current.group).saturating_add(1), 0)),
        WindowKind::PercentRank => {
            let others = len.saturating_sub(1);
            Value::Approx(if others == 0 { 0.0_f64 } else { real(current.peers.0) / real(others) })
        }
        WindowKind::CumeDist => Value::Approx(real(current.peers.1) / real(len.max(1))),
        WindowKind::Ntile(_) => match tiles {
            Count::Of(n) => count(tile(current.row, len, n).saturating_sub(1)),
            Count::Null | Count::Unused => Value::Null,
        },
        WindowKind::Aggregate(_) | WindowKind::Offset(_) | WindowKind::Value(_) => {
            return Err(broken("a set function or a value function computed as a rank"));
        }
    })
}

/// The `f64` nearest to the count `n`.
fn real(n: usize) -> f64 {
    Decimal::new(i128::try_from(n).unwrap_or(i128::MAX), 0).to_f64()
}

// Unit tests: ranks and peers (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
