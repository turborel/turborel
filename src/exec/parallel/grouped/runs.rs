//! \[DESIGN\] (RULES.md D1125) What a partition of a grouping run in
//! parallel gives of its groups, and the order they are given in. A
//! partition gave a row of values of each of its groups, all made before
//! the first was read, in memory no grant was charged. A grouping with
//! no HAVING condition nor second grouping now gives its table's groups
//! as a batch (`table/given.rs`), the table's own vectors, which the
//! grant holds already, and a row is made of a group only as it is given.
//! Any other partition gives rows, as before.
//!
//! The partitions' groups are given in the order of their first rows:
//! each run is in that order, and of the runs' groups next given the one
//! of the earliest first row is taken, as a merge of sorted runs takes
//! them. A run read to its end gives back what it held.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use crate::error::Result;
use crate::exec::broken;
use crate::storage::{Batch, Row};

/// A partition's groups, each with its first row's number, in the order
/// of those numbers: finished rows, or a table's groups as a batch and
/// the numbers beside it.
pub(super) enum Run {
    Rows(Vec<(u64, Row)>),
    Columns(Batch, Vec<u64>),
}

impl Run {
    /// Whether it holds no group.
    pub(super) fn is_empty(&self) -> bool {
        match self {
            Run::Rows(rows) => rows.is_empty(),
            Run::Columns(_, firsts) => firsts.is_empty(),
        }
    }

    /// Its groups as rows, each with its number, in its order.
    pub(super) fn into_rows(self) -> Vec<(u64, Row)> {
        match self {
            Run::Rows(rows) => rows,
            Run::Columns(batch, firsts) => firsts.iter().enumerate().map(|(g, &first)| (first, row_of(&batch, g))).collect(),
        }
    }
}

/// The row of group `g` of `batch`: each column's value of it.
fn row_of(batch: &Batch, g: usize) -> Row {
    (0..batch.columns.len()).map(|c| batch.cell(c, g).to_value()).collect()
}

/// A run being read: its rows left, or its batch and the group next
/// given.
enum Reading {
    Rows(std::vec::IntoIter<(u64, Row)>),
    Columns(Batch, Vec<u64>, usize),
}

impl Reading {
    /// Its next group, its number and row, made now of a batch's; none
    /// at its end, where what it held is given back.
    fn next(&mut self) -> Option<(u64, Row)> {
        let next = match self {
            Reading::Rows(rows) => rows.next(),
            Reading::Columns(batch, firsts, at) => {
                let g = *at;
                *at = at.saturating_add(1);
                firsts.get(g).map(|&first| (first, row_of(batch, g)))
            }
        };
        if next.is_none() {
            *self = Reading::Rows(Vec::new().into_iter());
        }
        next
    }
}

/// The groups of each partition, each partition's in the order of their
/// first rows, given in that order: of the partitions' first groups not
/// yet given, the earliest, as a merge of sorted runs takes them.
pub(super) struct Ordered {
    runs: Vec<Reading>,
    /// Each run's group next given, if any.
    heads: Vec<Option<Row>>,
    /// The first rows' numbers of the heads, each with its run, earliest
    /// first.
    order: BinaryHeap<Reverse<(u64, usize)>>,
}

impl Ordered {
    /// The groups of the runs `runs`, each in order.
    pub(super) fn of(runs: Vec<Run>) -> Ordered {
        let mut ordered = Ordered { runs: Vec::new(), heads: Vec::new(), order: BinaryHeap::with_capacity(runs.len()) };
        for (at, run) in runs.into_iter().enumerate() {
            let mut run = match run {
                Run::Rows(rows) => Reading::Rows(rows.into_iter()),
                Run::Columns(batch, firsts) => Reading::Columns(batch, firsts, 0),
            };
            let head = run.next().map(|(first, row)| {
                ordered.order.push(Reverse((first, at)));
                row
            });
            ordered.runs.push(run);
            ordered.heads.push(head);
        }
        ordered
    }
}

impl Iterator for Ordered {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        let Reverse((_, at)) = self.order.pop()?;
        let (Some(head), Some(run)) = (self.heads.get_mut(at), self.runs.get_mut(at)) else {
            return Some(Err(broken("a partition's groups are missing")));
        };
        let row = head.take();
        *head = run.next().map(|(first, next)| {
            self.order.push(Reverse((first, at)));
            next
        });
        row.map(Ok)
    }
}

// Unit tests: runs of rows and of batches are given in one order (RULES.md
// R-TEST-1).
#[cfg(test)]
mod tests;
