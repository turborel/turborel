//! [ISO/IEC 9075-2:2023 §7.15, §6.10] Window functions, run: the rows of
//! the input, in order of their window partitioning and ordering (the
//! planner sorts them, unless they come so), read one partition at a time
//! [GR3, GR4]. Each partition's rows are held, as are, of each, its peer
//! group's number, its range key and its calls' arguments, up to a part of
//! the work area each and spilled past it (RULES.md D38). Then each row's
//! value of each call is found, its frame's bounds from pointers that only
//! move forward (`frame.rs`), and the rows given with their values, before
//! the next partition is read.
//!
//! The rows a window function compares are those of its partition in its
//! ordering: no partition's rows depend on another's. Peers are given in the
//! order the sort gives them, one for every window of one partitioning and
//! ordering, as one node computes them all (US035).
//!
//! Each row is taken only while the statement is not cancelled (RULES.md
//! D44).

mod aggregate;
mod frame;
mod reader;

use std::cmp::Ordering;
use std::sync::Arc;

use self::aggregate::Framed;
use self::frame::{Bounds, Current, Partition};
use self::reader::{Placed, Reader};
use super::work::Work;
use super::{HeldRows, RowIter, Runner, broken, column, eval, execute, failed, warn};
use crate::db::State;
use crate::error::{Result, SqlState};
use crate::logical::{SortKey, Window, WindowKind};
use crate::physical::Plan;
use crate::spill::ListSpace;
use crate::storage::Row;
use crate::txn::{TxId, checked};
use crate::value::{Decimal, Value};

/// How many parts of the work area a partition's rows, their values, the
/// places' values its frames are found by, and the trees of states each
/// take at most.
const PARTS: usize = 4;

/// The rows of `plan`, window functions over its input.
#[inline(never)]
pub(super) fn windowed(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    let Plan::Window { window, .. } = plan else { return failed(broken("a window is not one")) };
    let (w, input) = window.split();
    let partition = w.partition.iter().map(|&c| SortKey::ascending(c)).collect();
    let rows = execute(input, db, tx);
    let (db_held, runner) = (Arc::clone(db), Runner::new(db, tx));
    let given = Windowed { w, partition, rows, runner, db: db_held, tx, bounds: None, next: None, given: None, done: false };
    Box::new(checked(given, db.tx.stop_of(tx)))
}

/// The rows of one partition, given with their calls' values.
type Given = Box<dyn Iterator<Item = Result<Row>> + Send>;

/// Window functions over rows in order, a partition at a time.
struct Windowed {
    w: Window<()>,
    /// The partitioning columns, as keys a row's are compared by.
    partition: Vec<SortKey>,
    rows: RowIter,
    runner: Runner,
    db: Arc<State>,
    tx: TxId,
    /// Each call's frame bounds, their offsets evaluated as the first row
    /// is read [§7.15 GR5b].
    bounds: Option<Vec<Bounds>>,
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
            let order = self.w.order.first().map(SortKey::order);
            let bounds = self.w.calls.iter().map(|c| Bounds::of(&c.frame, order, &self.runner)).collect::<Result<_>>()?;
            self.bounds = Some(bounds);
        }
        let work = Work::of(&self.db, self.tx);
        let space = ListSpace { limit: part_of(work.limit()), ..self.db.list_space() };
        // [DESIGN] Each reader of the places' values keeps a segment of
        // them: the current row's, the peers', and each call's two bounds'
        // and its rows'.
        let readers = self.w.calls.len().saturating_mul(3).saturating_add(2);
        let keyed = ListSpace { limit: space.limit.checked_div(readers).unwrap_or_default().max(1), ..space.clone() };
        let (rows, mut part) = self.read(first, (space.clone(), keyed))?;
        let tree = space.limit.checked_div(self.w.calls.len()).unwrap_or_default();
        let values = self.values(&mut part, (space, tree), &work)?;
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
    fn read(&mut self, first: Row, (space, keyed): (ListSpace, ListSpace)) -> Result<(HeldRows<Row>, Partition)> {
        let parted = self.partition.iter().map(|k| column(&first, k.column).cloned()).collect::<Result<Vec<_>>>()?;
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
            keys.push(Placed { group, values: self.arguments(key, &row)? })?;
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
        Ok((rows, Partition { keys, len, nulls: nulls.unwrap_or((len, len)) }))
    }

    /// The values a row's frames are found by: its range key `key`, then
    /// each call's argument of `row`, null if it has none [§10.9 GR4].
    fn arguments(&self, key: Value, row: &Row) -> Result<Row> {
        let mut values = Vec::with_capacity(self.w.calls.len().saturating_add(1));
        values.push(key);
        for call in &self.w.calls {
            let arg = match &call.kind {
                WindowKind::Aggregate(a) => a.arg.as_ref(),
                WindowKind::RowNumber | WindowKind::Rank | WindowKind::DenseRank | WindowKind::PercentRank | WindowKind::CumeDist => None,
            };
            values.push(arg.map_or(Ok(Value::Null), |e| eval(e, row, &self.runner))?);
        }
        Ok(values)
    }

    /// Of each row of `part`, its calls' values, held in `space`, a tree
    /// of states each taking at most `tree` bytes, a DISTINCT set
    /// function's values kept in `work`.
    fn values(&mut self, part: &mut Partition, (space, tree): (ListSpace, usize), work: &Work) -> Result<HeldRows<Row>> {
        let Some(bounds) = self.bounds.as_mut() else { return Err(broken("a window's frames were not bound")) };
        bounds.iter_mut().for_each(Bounds::reset);
        let mut framed = Vec::with_capacity(self.w.calls.len());
        for (n, (call, b)) in self.w.calls.iter().zip(bounds.iter()).enumerate() {
            framed.push(match &call.kind {
                WindowKind::Aggregate(a) => Some(Framed::new(a, (b, n.saturating_add(1)), part, tree, work)?),
                WindowKind::RowNumber | WindowKind::Rank | WindowKind::DenseRank | WindowKind::PercentRank | WindowKind::CumeDist => None,
            });
        }
        let (mut out, mut peers) = (HeldRows::new(space), Peers::default());
        for row in 0..part.len {
            let current = current(row, part, &mut peers)?;
            let mut values = Vec::with_capacity(framed.len());
            for ((call, b), f) in self.w.calls.iter().zip(bounds.iter_mut()).zip(&mut framed) {
                values.push(match f {
                    Some(f) => aggregated(f, (b, &current), part, &self.runner)?,
                    None => rank(&call.kind, &current, part.len)?,
                });
            }
            out.push(values)?;
        }
        Ok(out)
    }
}

/// The value of the set function `f` over `current`'s frame of `bounds`,
/// a row of `part`.
fn aggregated(f: &mut Framed<'_>, (bounds, current): (&mut Bounds, &Current), part: &mut Partition, runner: &Runner) -> Result<Value> {
    let (value, nulls) = f.value(bounds.frame(current, part)?, part)?;
    // [ISO/IEC 9075:1992 §6.5 GR1b] a null left out warns.
    if nulls {
        warn(runner, SqlState::NULL_VALUE_ELIMINATED);
    }
    Ok(value)
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

/// [ISO/IEC 9075-2:2023 §9.23 SR5] The value of the rank function `kind`
/// of `current`, a row of a partition of `len` rows: ROW_NUMBER its
/// place, RANK 1 plus the rows before its peers, DENSE_RANK 1 plus the
/// peer groups before its own, each of COUNT's type (IV166); PERCENT_RANK
/// (RANK - 1) / (`len` - 1), or 0 of one row, and CUME_DIST the rows up to
/// its last peer over `len`, approximate (IV082, IV083).
fn rank(kind: &WindowKind, current: &Current, len: usize) -> Result<Value> {
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
        WindowKind::Aggregate(_) => return Err(broken("a set function computed as a rank")),
    })
}

/// The `f64` nearest to the count `n`.
fn real(n: usize) -> f64 {
    Decimal::new(i128::try_from(n).unwrap_or(i128::MAX), 0).to_f64()
}

// Unit tests: ranks and peers (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
