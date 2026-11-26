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
//! them, with the runs read back from the spill (RULES.md D1226). A run
//! read to its end gives back what it held. A partition whose groups go
//! to the spill gives them there as rows.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use super::Given;
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

    /// Its groups one at a time, each with its number, in its order: a
    /// batch's each made a row as it is given, and the batch given back
    /// at its end.
    fn given(self) -> Given {
        match self {
            Run::Rows(rows) => Box::new(rows.into_iter().map(Ok)),
            Run::Columns(batch, firsts) => {
                let mut held = Some((batch, firsts.into_iter().enumerate()));
                Box::new(std::iter::from_fn(move || {
                    let next = held.as_mut().and_then(|(batch, firsts)| firsts.next().map(|(g, first)| (first, row_of(batch, g))));
                    if next.is_none() {
                        held = None;
                    }
                    next.map(Ok)
                }))
            }
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

/// The groups of each partition, each partition's in the order of their
/// first rows, given in that order: of the partitions' first groups not
/// yet given, the earliest, as a merge of sorted runs takes them.
pub(super) struct Ordered {
    runs: Vec<Given>,
    /// Each run's group next given, if any.
    heads: Vec<Option<Row>>,
    /// The first rows' numbers of the heads, each with its run, earliest
    /// first.
    order: BinaryHeap<Reverse<(u64, usize)>>,
}

impl Ordered {
    /// The groups of the runs `held` and `given`, each in order.
    ///
    /// # Errors
    ///
    /// As reading a run back fails.
    pub(super) fn of(held: Vec<Run>, given: Vec<Given>) -> Result<Ordered> {
        let held = held.into_iter().map(Run::given);
        let runs: Vec<Given> = held.chain(given).collect();
        let mut ordered = Ordered { heads: runs.iter().map(|_| None).collect(), order: BinaryHeap::with_capacity(runs.len()), runs };
        for at in 0..ordered.runs.len() {
            ordered.advance(at)?;
        }
        Ok(ordered)
    }

    /// Takes run `at`'s next group as its head, if it has one.
    ///
    /// # Errors
    ///
    /// As reading the run back fails.
    fn advance(&mut self, at: usize) -> Result<()> {
        let (Some(head), Some(run)) = (self.heads.get_mut(at), self.runs.get_mut(at)) else {
            return Err(broken("a partition's groups are missing"));
        };
        *head = run.next().transpose()?.map(|(first, next)| {
            self.order.push(Reverse((first, at)));
            next
        });
        Ok(())
    }
}

impl Iterator for Ordered {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        let Reverse((_, at)) = self.order.pop()?;
        let row = self.heads.get_mut(at).and_then(Option::take);
        if let Err(e) = self.advance(at) {
            return Some(Err(e));
        }
        row.map(Ok)
    }
}

// Unit tests: runs of rows and of batches are given in one order (RULES.md
// R-TEST-1).
#[cfg(test)]
mod tests;
