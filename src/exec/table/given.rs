//! \[DESIGN\] (RULES.md D1123) A table's groups given as one batch, a
//! group a row in the order the groups were made: the lanes of its keys
//! as columns (`keys/columns.rs`), then each set function's results
//! (`states/columns.rs`), and beside them each group's first row's
//! number. What groups the groups again, or reads them by batch, takes
//! them so with no row of values made of a group. Each value is the one
//! `GroupTable::finished` gives of that group and column.

use super::GroupTable;
use crate::error::Result;
use crate::storage::Batch;

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
}

// Unit tests: a batch of groups holds what their rows hold (RULES.md
// R-TEST-1).
#[cfg(test)]
mod tests;
