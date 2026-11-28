//! \[DESIGN\] (RULES.md D1266) A column store table's batches, each with
//! what its scan judged of it (`storage::Vetted`): of a batch of frozen
//! rows, what `run.rs` noted as it made the batch; of a batch of the
//! tree's rows, nothing, as the scan judges no conjunct of them
//! (`Batched::delta_record`).

use super::Batched;
use crate::error::Result;
use crate::storage::{Batch, Vetted};

/// \[DESIGN\] (RULES.md D1266) `Batched`'s batches, each with what it
/// judged of that batch.
pub(in crate::engine::rows) struct Vetting(pub(in crate::engine::rows) Batched);

impl Iterator for Vetting {
    type Item = Result<(Batch, Option<Vetted>)>;

    fn next(&mut self) -> Option<Self::Item> {
        // What is said is taken with the batch it is said of, so it is
        // never left to be read of another.
        let batch = self.0.step().transpose()?;
        Some(batch.map(|b| (b, self.0.vetted.take())))
    }
}
