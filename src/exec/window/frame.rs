//! [ISO/IEC 9075-2:2023 §7.15 GR5] The full window frame of each row of a
//! partition, found row after row: each bound a place in the partition,
//! from a pointer that only moves forward, as the bounds of the next row
//! are never before those of the row before it. ROWS counts rows, GROUPS
//! peer groups, and RANGE compares the range key with the current row's,
//! plus or minus the offset [GR5b i]. What the exclusion leaves out
//! splits the frame into at most three ranges [GR5b ii].

use std::cmp::Ordering;

use super::reader::{Placed, Reader};
use crate::error::{Code, Result, SqlState, fail_with};
use crate::exec::{HeldRows, Runner, broken, eval};
use crate::logical::{Bound, Exclusion, Frame, Units};
use crate::value::interval::shift_clamped;
use crate::value::{ArithOp, SortOrder, Value, arith};

/// The rows of a frame, as places: at most three ranges, from first to
/// last, apart and in order, each `(from, to)` of the places from `from`
/// up to `to`; an empty range is `EMPTY`.
pub(in crate::exec) type Ranges = [(usize, usize); 3];

/// A range of no places.
pub(in crate::exec) const EMPTY: (usize, usize) = (0, 0);

/// A partition, as its frames are found.
pub(in crate::exec) struct Partition {
    /// Of each row, in order, what its frames are found by.
    pub(in crate::exec) keys: HeldRows<Placed>,
    /// How many rows it has.
    pub(in crate::exec) len: usize,
    /// The places of the rows whose range key is null, a block at its
    /// start or its end [§10.10 GR1e ii]: `(len, len)` if there are none.
    pub(in crate::exec) nulls: (usize, usize),
}

impl Partition {
    /// The places of the rows whose range key is not null.
    fn not_null(&self) -> (usize, usize) {
        if self.nulls.0 == 0 { (self.nulls.1, self.len) } else { (0, self.nulls.0) }
    }

    /// The range key of the `place`th row, read by `reader`.
    fn key<'a>(&'a mut self, reader: &'a mut Reader, place: usize) -> Result<&'a Value> {
        reader.get(&mut self.keys, place)?.values.first().ok_or_else(|| broken("a window's row has no range key"))
    }
}

/// The row whose frame is found: its place, its peer group's number and
/// places, and its range key.
pub(in crate::exec) struct Current {
    /// Its place in the partition.
    pub(in crate::exec) row: usize,
    /// Its peer group's number.
    pub(in crate::exec) group: u64,
    /// The places of its peers, itself among them.
    pub(in crate::exec) peers: (usize, usize),
    /// Its range key.
    pub(in crate::exec) key: Value,
}

/// [GR5b] A frame bound, its offset evaluated: as a place.
#[derive(Debug, Clone)]
pub(in crate::exec) enum Edge {
    /// UNBOUNDED PRECEDING: the partition's first row.
    First,
    /// UNBOUNDED FOLLOWING: past the partition's last row.
    Last,
    /// CURRENT ROW of ROWS: the current row [GR5b i 2].
    Row,
    /// CURRENT ROW of RANGE or GROUPS: the current row's peers [GR5b i 1 B
    /// II, C II].
    Peers,
    /// An offset of ROWS: rows before (`back`) or after the current row.
    Rows {
        /// PRECEDING rather than FOLLOWING.
        back: bool,
        /// The offset.
        count: usize,
    },
    /// An offset of GROUPS: peer groups before or after the current row's.
    Groups {
        /// PRECEDING rather than FOLLOWING.
        back: bool,
        /// The offset.
        count: u64,
    },
    /// An offset of RANGE: rows whose range key is not before, or not
    /// after, the current row's moved by `by`.
    Range {
        /// PRECEDING rather than FOLLOWING.
        back: bool,
        /// The offset.
        by: Value,
        /// The order of the range key.
        order: SortOrder,
    },
}

/// One bound of a frame and where its pointer is.
struct Side {
    edge: Edge,
    /// Whether it is the frame's end, so its place is past the rows the
    /// frame takes.
    end: bool,
    /// Where its pointer is: never before where it was.
    at: usize,
    reader: Reader,
}

/// The bounds of a window function's frame, found row after row of a
/// partition.
pub(in crate::exec) struct Bounds {
    start: Side,
    end: Side,
    exclusion: Exclusion,
}

impl Bounds {
    /// The bounds `start` and `end`, leaving out what `exclusion` says.
    pub(in crate::exec) fn new(start: Edge, end: Edge, exclusion: Exclusion) -> Bounds {
        let side = |edge, is_end| Side { edge, end: is_end, at: 0, reader: Reader::default() };
        Bounds { start: side(start, false), end: side(end, true), exclusion }
    }

    /// The bounds of `frame`, its offsets evaluated, the range key in
    /// `order` if it has one.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22013 if an offset is negative or null [GR5b i 1 B I 1];
    /// an offset's evaluation's.
    pub(in crate::exec) fn of(frame: &Frame, order: Option<SortOrder>, runner: &Runner) -> Result<Bounds> {
        let start = edge(&frame.start, (frame.units, order), runner)?;
        let end = edge(&frame.end, (frame.units, order), runner)?;
        Ok(Bounds::new(start, end, frame.exclusion))
    }

    /// Begins a partition: each pointer at its first row.
    pub(in crate::exec) fn reset(&mut self) {
        self.start.at = 0;
        self.end.at = 0;
    }

    /// Whether its frames only grow: each from the partition's first row,
    /// nothing left out.
    pub(in crate::exec) fn grows(&self) -> bool {
        matches!(self.start.edge, Edge::First) && self.exclusion == Exclusion::NoOthers
    }

    /// The frame of `current`, a row of `part` after those it was found
    /// of in this partition.
    ///
    /// # Errors
    ///
    /// The spill area's, reading `part`; a bound's arithmetic's, as RANGE
    /// moves the range key [GR5b i 1 A].
    pub(in crate::exec) fn frame(&mut self, current: &Current, part: &mut Partition) -> Result<Ranges> {
        let from = self.start.place(current, part)?;
        let to = self.end.place(current, part)?;
        Ok(excluded((from, to), current, self.exclusion))
    }
}

impl Side {
    /// The place of its bound for `current`: of its frame's first row, or
    /// past its last.
    fn place(&mut self, current: &Current, part: &mut Partition) -> Result<usize> {
        let (row, len) = (current.row, part.len);
        let beyond = usize::from(self.end);
        Ok(match &self.edge {
            Edge::First => 0,
            Edge::Last => len,
            Edge::Row => row.saturating_add(beyond).min(len),
            Edge::Peers => {
                if self.end {
                    current.peers.1
                } else {
                    current.peers.0
                }
            }
            Edge::Rows { back, count } => counted(row.saturating_add(beyond), (*back, *count), len),
            Edge::Groups { back, count } => {
                let (back, count) = (*back, *count);
                return self.groups(current, (back, count), part);
            }
            Edge::Range { back, by, order } => {
                let bound = if current.key.is_null() { None } else { Some(moved(&current.key, by, *back != order.descending)?) };
                let order = *order;
                return self.range(bound.as_ref(), order, part);
            }
        })
    }

    /// [GR5b i 3] The place of the first row of the peer group `count`
    /// groups before or after the current row's, or of a frame's end, past
    /// its last; the partition's first if there is no such group before,
    /// past its last if none after.
    fn groups(&mut self, current: &Current, (back, count): (bool, u64), part: &mut Partition) -> Result<usize> {
        let target = if back {
            let Some(target) = current.group.checked_sub(count) else { return Ok(0) };
            target
        } else {
            current.group.saturating_add(count)
        };
        while self.at < part.len {
            let group = self.reader.get(&mut part.keys, self.at)?.group;
            if passed(group.cmp(&target), self.end) {
                break;
            }
            self.at = self.at.saturating_add(1);
        }
        Ok(self.at)
    }

    /// [GR5b i 1 B, C] The place of the first row whose range key is not
    /// before `bound`, or of a frame's end, is after it, among the rows
    /// whose key is not null; of a current row whose key is null (`bound`
    /// none), the first of the rows whose key is null, or past their last.
    fn range(&mut self, bound: Option<&Value>, order: SortOrder, part: &mut Partition) -> Result<usize> {
        let Some(bound) = bound else { return Ok(if self.end { part.nulls.1 } else { part.nulls.0 }) };
        let (from, to) = part.not_null();
        self.at = self.at.max(from);
        while self.at < to {
            let key = part.key(&mut self.reader, self.at)?;
            if passed(order.cmp(key, bound), self.end) {
                break;
            }
            self.at = self.at.saturating_add(1);
        }
        Ok(self.at)
    }
}

/// Whether a pointer stops at a row of the order `order` to its bound's
/// target: the start at the first not before it, the end at the first
/// after it.
fn passed(order: Ordering, end: bool) -> bool {
    match order {
        Ordering::Less => false,
        Ordering::Equal => !end,
        Ordering::Greater => true,
    }
}

/// [GR5b i 2] The place `count` rows before (`back`) or after `place`,
/// never before the partition's first or past its last, of `len` rows.
fn counted(place: usize, (back, count): (bool, usize), len: usize) -> usize {
    if back { place.saturating_sub(count) } else { place.saturating_add(count).min(len) }
}

/// [GR5b i 1 B I 2 b ii, III 2 b ii] The range key `key` moved by `by`:
/// less if `subtract`. A datetime moved by a year-month interval to a day
/// its month has not is that month's last day [GR5b i 1 A].
fn moved(key: &Value, by: &Value, subtract: bool) -> Result<Value> {
    if let (Value::Datetime(d), Value::Interval(i)) = (key, by) {
        return shift_clamped(d, i, subtract);
    }
    arith(if subtract { ArithOp::Sub } else { ArithOp::Add }, key, by)
}

/// [GR5b ii] The rows from `from` up to `to`, those `exclusion` leaves out
/// of `current`'s frame taken away.
pub(in crate::exec) fn excluded((from, to): (usize, usize), current: &Current, exclusion: Exclusion) -> Ranges {
    let (row, (peers_from, peers_to)) = (current.row, current.peers);
    let cut = |a: usize, b: usize| {
        let (a, b) = (a.max(from), b.min(to));
        if a < b { (a, b) } else { EMPTY }
    };
    let after = row.saturating_add(1);
    match exclusion {
        Exclusion::NoOthers => [cut(from, to), EMPTY, EMPTY],
        Exclusion::CurrentRow => [cut(from, row), cut(after, to), EMPTY],
        Exclusion::Group => [cut(from, peers_from), cut(peers_to, to), EMPTY],
        Exclusion::Ties => [cut(from, peers_from), cut(row, after), cut(peers_to, to)],
    }
}

/// The edge of `bound`, of a frame of `units`, the range key in `order`.
fn edge(bound: &Bound, (units, order): (Units, Option<SortOrder>), runner: &Runner) -> Result<Edge> {
    let (back, offset) = match bound {
        Bound::UnboundedPreceding => return Ok(Edge::First),
        Bound::UnboundedFollowing => return Ok(Edge::Last),
        Bound::CurrentRow => {
            return Ok(match units {
                Units::Rows => Edge::Row,
                Units::Range | Units::Groups => Edge::Peers,
            });
        }
        Bound::Preceding(e) => (true, e),
        Bound::Following(e) => (false, e),
    };
    let by = eval(offset, &Vec::new(), runner)?;
    if by.is_null() || negative(&by) {
        let what = if by.is_null() { "null" } else { "negative" };
        return fail_with(
            Code::Data,
            SqlState::INVALID_PRECEDING_OR_FOLLOWING_SIZE,
            format!("[ISO/IEC 9075-2:2023 §7.15 GR5b i 1 B I 1] a window frame's offset is {what}"),
        );
    }
    Ok(match units {
        Units::Rows => Edge::Rows { back, count: usize::try_from(whole(&by)?).unwrap_or(usize::MAX) },
        Units::Groups => Edge::Groups { back, count: u64::try_from(whole(&by)?).unwrap_or(u64::MAX) },
        Units::Range => Edge::Range { back, by, order: order.ok_or_else(|| broken("a RANGE offset with no sort key"))? },
    })
}

/// Whether the offset `v` is negative.
fn negative(v: &Value) -> bool {
    match v {
        Value::Exact(d) => d.units < 0,
        Value::Approx(x) => *x < 0.0_f64,
        Value::DecFloat(d) => d.coefficient() < 0,
        Value::Interval(i) => i.units < 0,
        Value::Null
        | Value::Char(_)
        | Value::Binary(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Row(_)
        | Value::Array(_)
        | Value::Extension(_) => false,
    }
}

/// The offset of ROWS or GROUPS `v`, exact of scale 0 [§7.15 SR15b], not
/// negative.
fn whole(v: &Value) -> Result<i128> {
    if let Value::Exact(d) = v
        && d.scale == 0
    {
        return Ok(d.units);
    }
    Err(broken("a ROWS or GROUPS offset is not an exact number of scale 0"))
}

// Unit tests: frames found row after row against frames found alone
// (RULES.md R-TEST-1, R-TEST-5).
#[cfg(test)]
mod tests;
