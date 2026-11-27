//! \[DESIGN\] (RULES.md D1123) A table's groups given as one batch, a
//! group a row in the order the groups were made: the lanes of its keys
//! as columns (`keys/columns.rs`), then each set function's results
//! (`states/columns.rs`), and beside them each group's first row's
//! number. What groups the groups again, or reads them by batch, takes
//! them so with no row of values made of a group. Each value is the one
//! `GroupTable::finished` gives of that group and column.

use super::GroupTable;
use crate::error::Result;
use crate::exec::broken;
use crate::storage::{Batch, Column, Row};

impl GroupTable {
    /// Whether its groups are in the order of their first rows, as the
    /// groups of morsels merged in order are.
    pub(in crate::exec) fn in_order(&self) -> bool {
        self.firsts.is_sorted()
    }

    /// Its groups as a batch, its grouping columns then each set
    /// function's results, and each group's first row's number.
    ///
    /// # Errors
    ///
    /// As a set function's `finish`: the error `finished` raises, of the
    /// first group that fails whatever its set function.
    pub(in crate::exec) fn into_batch(self) -> Result<(Batch, Vec<u64>)> {
        let rows = self.len();
        let results = match self.states.iter().map(|states| states.results(rows)).collect::<Result<Vec<_>>>() {
            Ok(results) => results,
            Err(e) => return Err(self.finished().err().unwrap_or(e)),
        };
        let mut columns = self.keys.into_columns();
        columns.extend(results);
        Ok((Batch { columns, rows }, self.firsts))
    }

    /// \[DESIGN\] (RULES.md D1256) The results of its set function at
    /// `agg` of every group, as a column, a group a row, and no other
    /// column made.
    ///
    /// # Errors
    ///
    /// As the set function's `finish`; SQLCODE -910 of no such function.
    pub(in crate::exec) fn results_of(&self, agg: usize) -> Result<Column> {
        self.states.get(agg).ok_or_else(|| broken("a set function a table does not compute"))?.results(self.len())
    }

    /// \[DESIGN\] (RULES.md D1256) Finishes every group's result of each
    /// set function but those at `made`, keeping none, so a result that
    /// raises raises as it does when every group is given.
    ///
    /// # Errors
    ///
    /// As a set function's `finish`.
    pub(in crate::exec) fn checked_but(&self, made: &[usize]) -> Result<()> {
        let mut rest = self.states.iter().enumerate().filter(|(agg, _)| !made.contains(agg));
        rest.try_for_each(|(_, states)| states.checked(self.len()))
    }

    /// Group `g`'s row, its grouping values then each set function's
    /// result, as `finished` gives it.
    ///
    /// # Errors
    ///
    /// As a set function's `finish`.
    pub(in crate::exec) fn row(&self, g: usize) -> Result<Row> {
        let mut row = self.keys.values(g);
        row.reserve(self.states.len());
        for states in &self.states {
            row.push(states.finish(g)?);
        }
        Ok(row)
    }

    /// Each group's first row's number, in the order the groups were made.
    pub(in crate::exec) fn firsts(&self) -> &[u64] {
        &self.firsts
    }
}

// Unit tests: a batch of groups holds what their rows hold (RULES.md
// R-TEST-1).
#[cfg(test)]
mod tests;
