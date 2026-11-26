//! \[DESIGN\] (RULES.md D1123) The results of a table's set function
//! given as a column of a batch, a group a row: COUNT's counts as the
//! integers they are, every other function's as each group's finished
//! value, so each is the value its accumulator gives of the group.

use super::{States, Typed};
use crate::error::Result;
use crate::storage::{Column, Ints};
use crate::value::Value;

impl States {
    /// Each group's result, of the first `len` groups, as a column.
    ///
    /// # Errors
    ///
    /// As `Accumulator::finish`, of the first group that fails.
    pub(in crate::exec::table) fn results(&self, len: usize) -> Result<Column> {
        if let States::Typed(Typed::Count(counts)) = self {
            // [X3.135-1986 §5.8 GR4] A count is an exact number of scale
            // 0, as COUNT's accumulator finishes it.
            let units = (0..len).map(|g| counts.get(g).copied().unwrap_or(0)).collect();
            return Ok(Column::Ints(Ints::new(units, 0, None)));
        }
        (0..len).map(|g| self.finish(g)).collect::<Result<Vec<Value>>>().map(Column::Values)
    }
}
