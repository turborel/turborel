//! \[DESIGN\] (RULES.md D1129) A subplan a statement reads in several
//! places, computed once. Each place holds the subplan and the cell its
//! rows are kept in. The reader that opens first runs the subplan and
//! keeps its rows, within its work area and past it in the spill area
//! (RULES.md D38), and every reader reads them from there
//! (`exec/shared.rs`).
//!
//! The executor makes these nodes as it opens a filter whose condition
//! holds a subquery, of copies of the plans it runs. The planner makes
//! none, so no kept plan, no plan shown and no part run in parallel holds
//! one, and a cell is never read by two executions.

use super::Plan;
use crate::exec::SharedRows;

/// A place a shared subplan is read: its plan, and where its rows are
/// kept once a reader ran it.
#[derive(Debug, Clone)]
pub struct Shared {
    /// The subplan, which the reader that opens first runs.
    pub input: Plan,
    /// Its rows, once run, shared by every place it is read.
    pub rows: SharedRows,
}
