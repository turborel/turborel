//! \[DESIGN\] (RULES.md D1076) A LEFT outer join's probe of a table built
//! for a pipeline's morsels: each row of a batch paired with each built
//! row its key matches, as an inner join's probe pairs them (`probed`),
//! and a row no built row matches paired with none, its right columns
//! null [ISO/IEC 9075:1992 §7.5 GR4, GR5b], in the order of the batch's
//! rows, as the serial join gives them (`joined/probe.rs`).

use super::super::super::stream::Kept;
use super::{Pairs, SharedTable, pick};
use crate::error::SqlError;
use crate::exec::table::{KeyHashes, keys_equal};
use crate::storage::{BATCH_ROWS, Batch, NO_ROW};

impl SharedTable {
    /// The width of the operand's rows.
    pub(in crate::exec) fn width(&self) -> usize {
        self.width
    }

    /// The batches of the pairs of each row of `batch` at `rows`, or of
    /// all, its key columns at `left`, and each built row its key
    /// matches, or none if none does, in order: the rows paired before an
    /// error, then the error.
    pub(in crate::exec) fn probed_left(&self, (batch, rows): (&Batch, Option<&[u32]>), left: &[usize]) -> Vec<Result<Kept, SqlError>> {
        let given: Vec<usize> = (0..crate::storage::sel::kept_count(rows, batch.rows)).map(|n| crate::storage::sel::nth(rows, n)).collect();
        let mut out = Vec::new();
        let mut pairs: Pairs = (Vec::with_capacity(BATCH_ROWS), Vec::with_capacity(BATCH_ROWS));
        let paired = self.paired_left((batch, &given), left, &mut pairs, &mut out);
        if !pairs.0.is_empty() {
            out.push(Ok(self.gathered((batch, None), &mut pairs)));
        }
        out.extend(paired.err().map(Err));
        out
    }

    /// Pairs each row of `batch` at `given` with the built rows its key
    /// matches, or with none, into `pairs`, a batch of them gathered into
    /// `out` each time they are `BATCH_ROWS`.
    fn paired_left(
        &self,
        (batch, given): (&Batch, &[usize]),
        left: &[usize],
        pairs: &mut Pairs,
        out: &mut Vec<Result<Kept, SqlError>>,
    ) -> Result<(), SqlError> {
        let mut hashes = KeyHashes::default();
        hashes.of_batch((batch, Some(given)), (left, &self.nulls), self.hashing())?;
        for ((&r, &h), &miss) in given.iter().zip(&hashes.hashes).zip(&hashes.misses) {
            let mut any = false;
            for p in self.table.candidates(h).filter(|_| !miss) {
                if keys_equal((batch, left, r), (&self.right, &self.places, p), &self.nulls)? {
                    any = true;
                    pairs.0.push(pick(r));
                    pairs.1.push(pick(p));
                }
                if pairs.0.len() >= BATCH_ROWS {
                    out.push(Ok(self.gathered((batch, None), pairs)));
                }
            }
            if !any {
                // No built row: its right columns null (RULES.md D1083).
                pairs.0.push(pick(r));
                pairs.1.push(NO_ROW);
            }
        }
        Ok(())
    }
}
