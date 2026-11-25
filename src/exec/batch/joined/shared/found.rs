//! \[DESIGN\] (RULES.md D1075) A semi join's or an antijoin's probe of a
//! table built for a pipeline's morsels: of each row of a batch, whether
//! a built row matches it, its keys equal [X3.135-1986 §5.11], or not
//! distinct where a null matches a null [ISO/IEC 9075-2:2023 §8.15], and
//! what the join asks beyond them true of the pair. The first built row
//! that matches decides, and no other is looked at. No pair is gathered,
//! so a morsel's batch goes on as it came, its rows kept selected.

use super::SharedTable;
use crate::error::Result;
use crate::exec::table::{KeyHashes, keys_equal};
use crate::storage::Batch;

impl SharedTable {
    /// The column of the operand's rows each of its vectors holds.
    pub(in crate::exec) fn held_columns(&self) -> &[usize] {
        &self.columns
    }

    /// Of each row of `batch` at `rows`, its key columns at `left`,
    /// whether a built row matches it: one of its key of which `keeps`,
    /// given the row's place and the built batch and row, is true. The
    /// keys' hashes are made in `hashes`.
    ///
    /// # Errors
    ///
    /// As `keeps` fails; SQLCODE -910 if a value nests deeper than a type
    /// may.
    pub(in crate::exec) fn found(
        &self,
        (batch, rows): (&Batch, &[usize]),
        (left, hashes): (&[usize], &mut KeyHashes),
        mut keeps: impl FnMut(usize, (&Batch, usize)) -> Result<bool>,
    ) -> Result<Vec<bool>> {
        hashes.of_batch((batch, Some(rows)), (left, &self.nulls), self.hashing())?;
        let mut found = Vec::with_capacity(rows.len());
        for ((&r, &h), &miss) in rows.iter().zip(&hashes.hashes).zip(&hashes.misses) {
            let mut any = false;
            for p in self.table.candidates(h).filter(|_| !miss) {
                if keys_equal((batch, left, r), (&self.right, &self.places, p), &self.nulls)? && keeps(r, (&self.right, p))? {
                    any = true;
                    break;
                }
            }
            found.push(any);
        }
        Ok(found)
    }
}
