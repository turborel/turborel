//! \[DESIGN\] (RULES.md D491) An index join probing a unique index by
//! batches of its outer rows: a batch read, each row's probe bound, the
//! probes sorted by their keys' values and each key searched for once, in
//! that order, its row fetched then too, so the index's pages and the
//! table's are read in key order and a key that repeats among the outer
//! rows is found once; then each outer row joined, in its order, with the
//! rows found for its key, as it would have been probing alone. A failure
//! binding a row's probe, searching for its key or fetching its row is
//! raised at that row, after the rows before it, as probing alone raises
//! it. A batch starts small and doubles, so a query that reads few rows
//! reads few beyond them. Of a unique index whose every column the probe
//! fixes, each key finds one row at most, so a batch holds no more rows
//! than it has outer rows.

use std::cmp::Ordering;
use std::collections::VecDeque;

use super::Probed;
use crate::catalog::IndexDesc;
use crate::error::Result;
use crate::exec::RowIter;
use crate::index::{Key, Probe};
use crate::physical::JoinProbe;
use crate::storage::{Row, RowId};
use crate::value::CompOp;
use crate::value::compare;

/// \[DESIGN\] The outer rows of a first batch: few, so a query that reads
/// a few rows probes for few beyond them.
const FIRST_ROWS: usize = 16;

/// \[DESIGN\] The most outer rows of a batch: enough that its keys' sorted
/// searches share the index's pages, few enough that its rows held are
/// small beside a work area.
const MOST_ROWS: usize = 1024;

/// What a key found: each entry, with its row if the version read has one,
/// and the failure that ended the search, if one did.
type Found = (Vec<((Key, RowId), Option<Row>)>, Option<crate::error::SqlError>);

/// An index join's outer rows read by batch, each with the number of what
/// its key found, none of a row whose probe has no key; what each key
/// found, or the failure binding a row's probe, with the outer rows yet to
/// take it; the failure that ended the outer rows, after them; and the
/// rows of the next batch.
pub(super) struct Batched {
    queue: VecDeque<(Row, Option<usize>)>,
    found: Vec<(Result<Found>, usize)>,
    failed: Option<crate::error::SqlError>,
    ended: bool,
    rows: usize,
}

/// One outer row joined: the row, what its key found.
pub(super) type Outer = (Row, Found);

impl Batched {
    /// An index join's batches, if it probes by `probe` the index `index`
    /// and each key it binds fixes every column of the unique index.
    pub(super) fn of(probe: &JoinProbe, index: &IndexDesc) -> Option<Batched> {
        let JoinProbe::Prefix(columns) = probe else { return None };
        let plain = index.expressions.is_empty() && index.collations.iter().all(Option::is_none);
        let whole = index.unique && plain && columns.len() == index.columns.len();
        whole.then(|| Batched { queue: VecDeque::new(), found: Vec::new(), failed: None, ended: false, rows: FIRST_ROWS })
    }

    /// The next outer row of `left` and what its key found in `table`,
    /// by `probe`; none past the last.
    ///
    /// # Errors
    ///
    /// As reading `left`, binding the row's probe, or searching for its
    /// key fails, raised at that row.
    pub(super) fn next(&mut self, left: &mut RowIter, (table, probe): (&mut Probed, &JoinProbe)) -> Result<Option<Outer>> {
        if self.queue.is_empty() && !self.ended {
            self.fill(left, (table, probe));
        }
        let Some((row, at)) = self.queue.pop_front() else { return self.failed.take().map_or(Ok(None), Err) };
        let Some((found, uses)) = at.and_then(|at| self.found.get_mut(at)) else { return Ok(Some((row, (Vec::new(), None)))) };
        // The last row to take what a key found takes it; each before, a copy.
        *uses = uses.saturating_sub(1);
        let taken = if *uses == 0 { std::mem::replace(found, Ok((Vec::new(), None))) } else { found.clone() };
        Ok(Some((row, taken?)))
    }

    /// Reads the next batch of `left`, up to its size, the size doubled
    /// for the next.
    fn read(&mut self, left: &mut RowIter) -> Vec<Row> {
        let mut rows = Vec::with_capacity(self.rows);
        while rows.len() < self.rows {
            match left.next() {
                Some(Ok(row)) => rows.push(row),
                Some(Err(e)) => {
                    self.failed = Some(e);
                    self.ended = true;
                    break;
                }
                None => {
                    self.ended = true;
                    break;
                }
            }
        }
        self.rows = self.rows.saturating_mul(2).min(MOST_ROWS);
        rows
    }

    /// Reads the next batch of `left` and finds its keys in key order.
    fn fill(&mut self, left: &mut RowIter, (table, probe): (&mut Probed, &JoinProbe)) {
        table.ready.clear();
        self.found.clear();
        let rows = self.read(left);
        let width = table.ix.columns.len();
        let bound: Vec<Result<Option<Probe>>> = rows.iter().map(|row| probe.bind(row, width)).collect();
        let mut slots: Vec<Option<usize>> = Vec::with_capacity(bound.len());
        let mut order: Vec<(usize, &Probe, &[crate::value::Value])> = Vec::with_capacity(bound.len());
        for (i, b) in bound.iter().enumerate() {
            let slot = match b {
                Ok(Some(p)) => match values(p) {
                    Some(key) => {
                        order.push((i, p, key));
                        None
                    }
                    // A probe of no values to sort by is searched for alone.
                    None => Some(self.kept(searched(table, p))),
                },
                Ok(None) => None,
                Err(e) => Some(self.kept(Err(e.clone()))),
            };
            slots.push(slot);
        }
        order.sort_by(|(_, _, a), (_, _, b)| compared(a, b));
        let mut last: Option<(&[crate::value::Value], usize)> = None;
        for (i, p, key) in order {
            let same = last.filter(|(before, _)| compared(before, key) == Ordering::Equal);
            let at = match same {
                Some((_, at)) => self.shared(at),
                None => self.kept(searched(table, p)),
            };
            if let Some(slot) = slots.get_mut(i) {
                *slot = Some(at);
            }
            last = Some((key, at));
        }
        // A row of no probe, a null among its keys, finds nothing.
        self.queue.extend(rows.into_iter().zip(slots));
    }

    /// The number of `found`, kept for one outer row.
    fn kept(&mut self, found: Result<Found>) -> usize {
        self.found.push((found, 1));
        self.found.len().saturating_sub(1)
    }

    /// `at`, what a key found, taken by one more outer row.
    fn shared(&mut self, at: usize) -> usize {
        if let Some((_, uses)) = self.found.get_mut(at) {
            *uses = uses.saturating_add(1);
        }
        at
    }
}

/// The values a probe's keys equal: of a prefix, or of one column equal
/// to a value.
fn values(probe: &Probe) -> Option<&[crate::value::Value]> {
    if let Probe::Compare { op: CompOp::Eq, value } = probe {
        return Some(std::slice::from_ref(value));
    }
    let Probe::Prefix { equal, then: None } = probe else { return None };
    Some(equal)
}

/// The order of two probes' keys, value by value.
fn compared(a: &[crate::value::Value], b: &[crate::value::Value]) -> Ordering {
    a.iter().zip(b).map(|(x, y)| compare(x, y).unwrap_or(Ordering::Equal)).find(|o| o.is_ne()).unwrap_or(Ordering::Equal)
}

/// What `probe` finds in `table`: its entries, each with its row, up to
/// the failure that ends them.
fn searched(table: &mut Probed, probe: &Probe) -> Result<Found> {
    let mut kept = Vec::new();
    for entry in table.searched(probe)? {
        let Ok((key, id)) = entry else { return Ok((kept, entry.err())) };
        match table.fetched(&key, id) {
            Ok(row) => kept.push(((key, id), row)),
            Err(e) => return Ok((kept, Some(e))),
        }
    }
    Ok((kept, None))
}
