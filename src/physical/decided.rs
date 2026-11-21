//! \[DESIGN\] (RULES.md D85) Conjuncts an index scan decides of its entries,
//! and what the node above a filter reads of its rows, which lets the
//! scan decide an entry without reading its row.

use super::{Built, PhysicalPlanner};
use crate::logical::{Cond, LogicalPlan, SortKey};

/// The conjunct an index scan decides of each entry, rather than its
/// filter (`Plan::IndexScan`): a comparison of a call of the index's
/// column with a constant, decided of an entry that is one point by the
/// call's function from the point (`ScalarFunction::point_compared`),
/// of any other by the call on its row.
#[derive(Debug, Clone)]
pub struct Decided {
    /// The conjunct.
    pub cond: Cond,
    /// The table's column the call is of, the index's.
    pub column: usize,
}

impl PhysicalPlanner<'_> {
    /// `input` built for a node reading only `reads` of its rows: a
    /// filter over a scan is told so (`TableRead::above`); any other
    /// input is built as it is.
    pub(super) fn build_input(&self, input: &LogicalPlan, want: &[SortKey], reads: &[usize]) -> Built {
        if let LogicalPlan::Filter { cond, input: scan } = input
            && matches!(**scan, LogicalPlan::Scan { .. })
        {
            // The filter divides a row goal by the share of rows it keeps
            // (`goals.rs`).
            return self.filter(cond, scan, want, Some(reads));
        }
        self.build(input, want)
    }
}
