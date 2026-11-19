//! \[EXTENSION\] DROP TABLE of a partitioned table or a partition
//! (RULES.md D153). A partitioned table's partitions are its rows, so each
//! goes with it, CASCADE or not; a partition dropped alone takes its rows
//! out of its tree, as DETACH and a drop would.

use super::super::ddl::Dropped;
use super::super::{State, rule};
use crate::catalog::{QualifiedName, TableDesc};
use crate::error::Result;

impl State {
    /// The partitions of `qn`, if it is partitioned, dropped first, each as
    /// DROP TABLE drops it, `cascade` as given; of a partition, that no
    /// foreign key references a table its rows are rows of.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if one does; as each partition's drop.
    pub(in crate::db) fn drop_partitions(&mut self, qn: &QualifiedName, cascade: bool) -> Result<Dropped> {
        let p = self.catalog.partitioning();
        let above = p.ancestors(qn);
        let referenced = self.catalog.tables().into_iter().flat_map(TableDesc::foreign_keys).any(|(_, fk)| above.contains(&fk.table));
        if referenced {
            return Err(rule(format!("[EXTENSION] a foreign key references a table {qn} is a partition of, so it is not dropped alone")));
        }
        let children: Vec<QualifiedName> = p.children(qn).into_iter().map(|d| d.table.clone()).collect();
        let mut dropped = Dropped::default();
        for child in &children {
            if self.catalog.table(child).is_some() {
                dropped.absorb(self.drop_table(child, cascade)?);
            }
        }
        Ok(dropped)
    }
}
