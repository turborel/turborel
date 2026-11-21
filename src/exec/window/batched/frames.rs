//! [ISO/IEC 9075-2:2023 §7.15 GR5] The full window frame of every row of a
//! partition held as vectors, found at once: each bound a place of the
//! partition, as `frame.rs` finds it row after row. ROWS counts rows,
//! GROUPS peer groups (each row's group's place among the group starts),
//! and RANGE compares the range key with the current row's moved by the
//! offset [GR5b i 1], its pointers only moving forward as the rows do.
//! What the exclusion leaves out splits a frame into at most three ranges
//! [GR5b ii] (`frame::excluded`).

use super::super::frame::{Current, Edge, Ranges, counted, excluded, moved, passed};
use crate::error::Result;
use crate::logical::Exclusion;
use crate::value::{SortOrder, Value};

/// A partition's rows, as its frames and ranks are found: of each row its
/// peer group's number, and where each group begins.
pub(in crate::exec::window) struct Part {
    /// How many rows it has.
    pub(in crate::exec::window) len: usize,
    /// Of each row, its peer group's number, from 0.
    pub(in crate::exec::window) group: Vec<u32>,
    /// Where each peer group begins, then `len`.
    pub(in crate::exec::window) starts: Vec<usize>,
}

impl Part {
    /// Of `len` rows, a peer group beginning at each of `starts`, the first
    /// at 0, ascending.
    pub(in crate::exec::window) fn of(len: usize, mut starts: Vec<usize>) -> Part {
        starts.push(len);
        let mut group = Vec::with_capacity(len);
        for (g, pair) in (0_u32..).zip(starts.windows(2)) {
            if let [from, to] = pair {
                group.resize(group.len().saturating_add(to.saturating_sub(*from)), g);
            }
        }
        Part { len, group, starts }
    }

    /// The number of row `row`'s peer group.
    pub(in crate::exec::window) fn group_of(&self, row: usize) -> usize {
        self.group.get(row).map_or(0, |&g| usize::try_from(g).unwrap_or(usize::MAX))
    }

    /// The places of the peers of row `row`, itself among them.
    pub(in crate::exec::window) fn peers(&self, row: usize) -> (usize, usize) {
        let g = self.group_of(row);
        (self.start(g), self.start(g.saturating_add(1)))
    }

    /// Where peer group `g` begins: past the last row if there is none.
    fn start(&self, g: usize) -> usize {
        self.starts.get(g).copied().unwrap_or(self.len)
    }

    /// The row `row` as `frame.rs` sees it, its range key left null.
    pub(in crate::exec::window) fn current(&self, row: usize) -> Current {
        let group = u64::try_from(self.group_of(row)).unwrap_or(u64::MAX);
        Current { row, group, peers: self.peers(row), key: Value::Null }
    }
}

/// The range keys of a partition's rows and where those null are, a block
/// at its start or its end [§10.10 GR1e ii]: `(len, len)` if none is.
pub(in crate::exec::window) type Keys<'a> = (&'a [Value], (usize, usize));

/// The frame of each row of `part`, of the bounds `(start, end,
/// exclusion)`, its range keys `keys` if a bound is a RANGE offset.
///
/// # Errors
///
/// A bound's arithmetic's, as RANGE moves the range key [GR5b i 1 A].
pub(in crate::exec::window) fn frames(
    (start, end, exclusion): (&Edge, &Edge, Exclusion),
    part: &Part,
    keys: Option<Keys<'_>>,
) -> Result<Vec<Ranges>> {
    let starts = places(start, false, part, keys)?;
    let ends = places(end, true, part, keys)?;
    Ok(starts.into_iter().zip(ends).enumerate().map(|(row, bounds)| excluded(bounds, &part.current(row), exclusion)).collect())
}

/// The place of `edge` of each row of `part`: of its frame's first row, or
/// past its last if `end`.
fn places(edge: &Edge, end: bool, part: &Part, keys: Option<Keys<'_>>) -> Result<Vec<usize>> {
    let (len, beyond) = (part.len, usize::from(end));
    let each = |f: &dyn Fn(usize) -> usize| (0..len).map(f).collect::<Vec<usize>>();
    Ok(match edge {
        Edge::First => vec![0; len],
        Edge::Last => vec![len; len],
        Edge::Row => each(&|row| row.saturating_add(beyond).min(len)),
        Edge::Peers => each(&|row| if end { part.peers(row).1 } else { part.peers(row).0 }),
        Edge::Rows { back, count } => each(&|row| counted(row.saturating_add(beyond), (*back, *count), len)),
        Edge::Groups { back, count } => each(&|row| groups(part, row, (*back, *count), end)),
        Edge::Range { back, by, order } => match keys {
            Some(keys) => ranged(keys, (*back, by, *order), end)?,
            None => return Err(crate::exec::broken("a RANGE offset with no range keys")),
        },
    })
}

/// [GR5b i 3] The place of the first row of the peer group `count` groups
/// before or after row `row`'s, or of a frame's `end`, past its last; the
/// partition's first if there is no such group before, past its last if
/// none after.
fn groups(part: &Part, row: usize, (back, count): (bool, u64), end: bool) -> usize {
    let g = u64::try_from(part.group_of(row)).unwrap_or(u64::MAX);
    let target = if back {
        let Some(target) = g.checked_sub(count) else { return 0 };
        target
    } else {
        g.saturating_add(count)
    };
    let target = usize::try_from(target).unwrap_or(usize::MAX).saturating_add(usize::from(end));
    part.start(target)
}

/// [GR5b i 1 B, C] Of each row, the place of the first row whose range key
/// is not before its own moved by `by`, or of a frame's `end`, is after
/// it, among the rows whose key is not null; of a row whose key is null,
/// the first of the rows whose key is null, or past their last.
fn ranged((keys, nulls): Keys<'_>, (back, by, order): (bool, &Value, SortOrder), end: bool) -> Result<Vec<usize>> {
    let (from, to) = if nulls.0 == 0 { (nulls.1, keys.len()) } else { (0, nulls.0) };
    let (mut at, mut out) = (from, Vec::with_capacity(keys.len()));
    for key in keys {
        if key.is_null() {
            out.push(if end { nulls.1 } else { nulls.0 });
            continue;
        }
        let bound = moved(key, by, back != order.descending)?;
        while let Some(k) = keys.get(at).filter(|_| at < to) {
            if passed(order.cmp(k, &bound), end) {
                break;
            }
            at = at.saturating_add(1);
        }
        out.push(at);
    }
    Ok(out)
}

// Unit tests: frames found at once against frames found row after row
// (RULES.md R-TEST-1, R-TEST-5).
#[cfg(test)]
mod tests;
