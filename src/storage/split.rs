//! \[DESIGN\] A scan split into morsels (RULES.md D290): parts of a
//! table's rows that the executor's workers read apart, each at once with
//! the others, as Leis, Boncz, Kemper and Neumann, "Morsel-Driven
//! Parallelism" (2014), cut a pipeline's input. Together the morsels give
//! every row the scan they split gives, once each, as the transaction
//! that asked for them sees it.

use super::{Batches, Rows};
use crate::value::Value;

/// \[DESIGN\] A scan of a table split into morsels (`TableStorage::split`).
/// It holds what the scan holds, its reader and the table's parts as they
/// were as it was split, so each morsel reads the rows the whole scan
/// would, whichever thread reads it and whenever.
pub trait Split: Send + Sync {
    /// How many morsels it has.
    fn morsels(&self) -> usize;

    /// The rows of morsel `n`, each with its id, in the order the scan
    /// gives them, of a table by row id, of an index by key; none of
    /// a morsel past the last. A row the engine cannot read is an error
    /// item, as of `TableStorage::scan`.
    fn morsel(&self, n: usize) -> Rows;

    /// \[DESIGN\] (RULES.md D354, D366) The rows of morsel `n` by batch, as
    /// the scan by batch (`TableStorage::scan_batches`) reads them, of the
    /// columns asked for alone, those of `late` late if it can, if its
    /// engine reads them so: none, the default, if not. A morsel's batches
    /// hold the rows `morsel` gives, in its order.
    fn morsel_batches(&self, n: usize, late: &[usize]) -> Option<Batches> {
        let _ = (n, late);
        None
    }

    /// \[DESIGN\] (RULES.md D1121) The least and greatest values not null
    /// of column `column`, by its place in the table, that morsel `n`'s rows
    /// may hold, as zones kept of its rows bound them, the values its scan
    /// gives each within them: `Some(None)` if none of them holds one; none,
    /// the default, if it cannot bound them.
    fn bounds(&self, n: usize, column: usize) -> Option<Option<(Value, Value)>> {
        let _ = (n, column);
        None
    }
}
