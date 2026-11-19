//! \[EXTENSION\] The nearest read of a partitioned table (RULES.md D156):
//! of each partition the read's condition admits, a nearest read of its
//! own index, each giving its first rows by distance, the reads appended.
//! The sort over them compares the exact measures of every candidate, so
//! the first rows of all are the first of the partitions' first rows. A
//! partition with no index that serves the read leaves the read to a scan
//! and a sort.

use super::super::{PhysicalPlanner, Plan};
use super::{NearestRead, Scanned};
use crate::index::NearestQuery;
use crate::logical::split_and;

impl PhysicalPlanner<'_> {
    /// The nearest reads `read` makes, its index chosen of `column` for
    /// `query`: of its table, or of each partition of it its condition
    /// admits, appended; none if a table has no index that serves it.
    pub(super) fn nearest_reads(&self, read: &NearestRead, column: usize, query: &NearestQuery) -> Option<Plan> {
        let tables = if self.db.catalog.partitioning().is_partitioned(&read.table) {
            if read.located {
                return None;
            }
            let mut conds = Vec::new();
            if let Some(c) = &read.condition {
                split_and(c.clone(), &mut conds);
            }
            self.db.admitted(&read.table, &conds)
        } else {
            vec![read.table.clone()]
        };
        let mut reads = Vec::new();
        for table in &tables {
            let desc = self.db.catalog.table(table)?;
            let scanned = Scanned { table, needed: &read.columns, condition: read.condition.as_ref() };
            let ((index, position), approximate, cost) = self.nearest_index((&scanned, desc), column, query, read.first)?;
            let one = NearestRead {
                table: table.clone(),
                index: index.name.clone(),
                position,
                method: index.method.clone(),
                approximate,
                cost,
                ..read.clone()
            };
            reads.push(Plan::Nearest(Box::new(one)));
        }
        reads.into_iter().rev().reduce(|after, one| Plan::Append { left: Box::new(one), right: Box::new(after) })
    }
}
