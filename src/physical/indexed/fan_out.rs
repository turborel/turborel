//! \[DESIGN\] Where the planner's estimates read a property graph's
//! statistics (RULES.md D117, `stats/graph.rs`): the rows a probe of an
//! edge table's index finds for one vertex, its fan-out, is the average
//! degree at the end whose endpoint columns the probe gives, if a graph
//! over the table knows it.

use super::super::PhysicalPlanner;
use crate::catalog::QualifiedName;

impl PhysicalPlanner<'_> {
    /// The rows of `table` each probe of an index finds whose columns
    /// `probed`, by their places in the table, are given: the average
    /// degree there, if they are an edge table's endpoint columns and a
    /// graph's statistics know it.
    pub(super) fn edge_fan_out(&self, table: &QualifiedName, probed: &[usize]) -> Option<f64> {
        self.db.edge_degree(table, probed).map(|d| d.average)
    }
}
