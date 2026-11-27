//! \[DESIGN\] (RULES.md D1280) A table's groups given to a second
//! grouping whose set functions merge the parts the table computed
//! (RULES.md D341). A part's column of states leaves the table as the
//! column it is, where `into_batch` made each group's part a binary
//! string for an accumulator of the second to read back. The second
//! grouping's table keeps each such function in a column of the same
//! kind, and the first's column is added to it slot by slot, each state
//! to the group its group of the first fell in (`states/parts.rs`).
//! Every other column is given and fed as before.

use super::states::{States, Typed};
use super::{GroupTable, Seed};
use crate::error::Result;
use crate::exec::batch::{Fed, Taken};
use crate::exec::{Runner, broken};
use crate::functions::MERGED;
use crate::logical::AggCall;
use crate::storage::{Batch, Column, Nulls, Row};

impl GroupTable {
    /// Of each of `aggs`, the set functions of a grouping over this
    /// table's groups that takes `taken` of them, the set function of
    /// this table whose column of states it merges as it is, if it does.
    /// It does if it merges the parts of a column this table keeps as a
    /// part's (`States::is_merged_whole`), and no function before it
    /// took that column.
    pub(in crate::exec) fn merged_by(&self, taken: &[Taken], aggs: &[AggCall]) -> Vec<Option<usize>> {
        let width = self.width();
        let mut from: Vec<Option<usize>> = Vec::with_capacity(aggs.len());
        for (agg, take) in aggs.iter().zip(taken) {
            let part = if let Taken::Column(at) = take { at.checked_sub(width) } else { None };
            let merges = agg.func.0.name() == MERGED && agg.filter.is_none();
            let part = part.filter(|at| merges && !from.contains(&Some(*at)));
            from.push(part.filter(|&at| self.states.get(at).is_some_and(States::is_merged_whole)));
        }
        from
    }

    /// Its groups as `into_batch` gives them, with the columns of states
    /// at `from` taken as they are, one for each place of `from` that
    /// names one. The batch holds nulls in the place of a column taken,
    /// which nothing reads.
    ///
    /// # Errors
    ///
    /// As `into_batch`, of the set functions whose results are made;
    /// SQLCODE -910 if `from` names a column that is no part's.
    pub(in crate::exec) fn into_parts(self, from: &[Option<usize>]) -> Result<(Batch, Vec<u64>, Vec<Option<Typed>>)> {
        let rows = self.len();
        let taken = |at: usize| from.contains(&Some(at));
        let made = |(at, states): (usize, &States)| if taken(at) { Ok(Column::Nulls(Nulls::new(rows))) } else { states.results(rows) };
        let results = match self.states.iter().enumerate().map(made).collect::<Result<Vec<_>>>() {
            Ok(results) => results,
            Err(e) => return Err(self.finished().err().unwrap_or(e)),
        };
        let mut held: Vec<Option<States>> = self.states.into_iter().map(Some).collect();
        let mut parts = Vec::with_capacity(from.len());
        for at in from {
            let Some(at) = at else {
                parts.push(None);
                continue;
            };
            let part = held.get_mut(*at).and_then(Option::take).and_then(States::into_part);
            parts.push(Some(part.ok_or_else(|| broken("a column of states taken as a part's that is none"))?));
        }
        let mut columns = self.keys.into_columns();
        columns.extend(results);
        Ok((Batch { columns, rows }, self.firsts, parts))
    }

    /// A table of no group yet of keys of `width` lanes computing `aggs`,
    /// hashed by `seed`, each function that merges one of `parts` kept
    /// in a column of that part's kind.
    pub(in crate::exec) fn merging(width: usize, aggs: &[AggCall], seed: &Seed, parts: &[Option<Typed>]) -> GroupTable {
        let mut table = GroupTable::new(width, aggs, seed, usize::MAX);
        for (states, part) in table.states.iter_mut().zip(parts) {
            if let Some(part) = part {
                *states = States::merging(part);
            }
        }
        table
    }

    /// Feeds the rows taken of `batch`, each a group of the table `parts`
    /// came of, as `feed` feeds them, and merges each of `parts` into its
    /// function's column, a row's state into the group that took the row.
    ///
    /// # Errors
    ///
    /// As `feed`, and as `States::merge_part`.
    pub(in crate::exec) fn feed_parts(
        &mut self,
        batch: &Batch,
        (taken, lanes): (&[Taken], &[Option<Fed<'_>>]),
        parts: &[Option<Typed>],
        (scratch, runner): (&mut Row, &Runner),
    ) -> Result<()> {
        if self.pending.groups.is_empty() {
            return Ok(());
        }
        let each = self.states.iter_mut().zip(&self.aggs).zip(taken.iter().zip(lanes)).zip(parts);
        for (((states, agg), (take, lane)), part) in each {
            match part {
                Some(part) => states.merge_part(part, (&self.pending.groups, &self.pending.read))?,
                None => states.feed((&self.pending, agg), (batch, take, lane), (scratch, runner))?,
            }
        }
        self.pending.given.clear();
        self.pending.read.clear();
        self.pending.groups.clear();
        Ok(())
    }
}

// Unit tests: a second grouping that merges the first's columns of states
// gives what one fed their parts as values gives (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
