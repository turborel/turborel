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

/// An index join's outer rows read by batch, each with what its key found
/// or the failure binding its probe; the failure that ended the outer
/// rows, after them; and the rows of the next batch.
pub(super) struct Batched {
    queue: VecDeque<(Row, Result<Found>)>,
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
        whole.then(|| Batched { queue: VecDeque::new(), failed: None, ended: false, rows: FIRST_ROWS })
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
        match self.queue.pop_front() {
            Some((row, found)) => Ok(Some((row, found?))),
            None => self.failed.take().map_or(Ok(None), Err),
        }
    }

    /// Reads the next batch of `left` and finds its keys in key order.
    fn fill(&mut self, left: &mut RowIter, (table, probe): (&mut Probed, &JoinProbe)) {
        table.ready.clear();
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
        let width = table.ix.columns.len();
        let bound: Vec<Result<Option<Probe>>> = rows.iter().map(|row| probe.bind(row, width)).collect();
        let mut found: Vec<Option<Result<Found>>> = bound.iter().map(|b| b.as_ref().err().map(|e| Err(e.clone()))).collect();
        // A probe of no values to sort by is searched for alone.
        for (i, b) in bound.iter().enumerate() {
            if let (Ok(Some(alone)), Some(slot)) = (b, found.get_mut(i))
                && values(alone).is_none()
            {
                *slot = Some(searched(table, Some(b)));
            }
        }
        let mut order: Vec<(usize, &[crate::value::Value])> = bound
            .iter()
            .enumerate()
            .filter_map(|(i, b)| b.as_ref().ok().and_then(Option::as_ref).and_then(values).map(|v| (i, v)))
            .collect();
        order.sort_by(|(_, a), (_, b)| compared(a, b));
        let mut last: Option<(&[crate::value::Value], Result<Found>)> = None;
        for (i, key) in order {
            let same = last.as_ref().filter(|(before, _)| compared(before, key) == Ordering::Equal).map(|(_, f)| f.clone());
            let result = same.unwrap_or_else(|| searched(table, bound.get(i)));
            if let Some(slot) = found.get_mut(i) {
                *slot = Some(result.clone());
            }
            last = Some((key, result));
        }
        // A row of no probe, a null among its keys, finds nothing.
        let none = || Ok((Vec::new(), None));
        self.queue.extend(rows.into_iter().zip(found).map(|(row, f)| (row, f.unwrap_or_else(none))));
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

/// What the probe `bound` finds in `table`: its entries, each with its row,
/// up to the failure that ends them.
fn searched(table: &mut Probed, bound: Option<&Result<Option<Probe>>>) -> Result<Found> {
    let Some(Ok(Some(probe))) = bound else { return Ok((Vec::new(), None)) };
    let mut kept = Vec::new();
    for entry in table.searched(probe)? {
        let Ok((key, id)) = entry else { return Ok((kept, entry.err())) };
        match table.row(Ok((key.clone(), id))) {
            Ok(row) => kept.push(((key, id), row)),
            Err(e) => return Ok((kept, Some(e))),
        }
    }
    Ok((kept, None))
}
