//! \[DESIGN\] A scan split into morsels (RULES.md D290): parts of a
//! table's rows that the executor's workers read apart, each at once with
//! the others, as Leis, Boncz, Kemper and Neumann, "Morsel-Driven
//! Parallelism" (2014), cut a pipeline's input. Together the morsels give
//! every row the scan they split gives, once each, as the transaction
//! that asked for them sees it.

use super::Rows;

/// \[DESIGN\] A scan of a table split into morsels (`TableStorage::split`).
/// It holds what the scan holds, its reader and the table's parts as they
/// were as it was split, so each morsel reads the rows the whole scan
/// would, whichever thread reads it and whenever.
pub trait Split: Send + Sync {
    /// How many morsels it has.
    fn morsels(&self) -> usize;

    /// The rows of morsel `n`, each with its id, in row id order; none of
    /// a morsel past the last. A row the engine cannot read is an error
    /// item, as of `TableStorage::scan`.
    fn morsel(&self, n: usize) -> Rows;
}
