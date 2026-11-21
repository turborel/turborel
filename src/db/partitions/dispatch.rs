//! \[EXTENSION\] The rows a global index names, read from their partitions
//! (RULES.md D157): a storage of the partitioned table that holds none of
//! its own, whose every row id is a partition's id and a row's id in it
//! (`global::packed`), so an index scan of a global index, and a key's
//! check through it, read each row from the partition it names. A row of
//! a partition no longer in the tree is none. Its rows are written to its
//! partitions, never through it.

use std::collections::HashMap;
use std::sync::Arc;

use super::super::{State, api};
use super::global::unpacked;
use crate::catalog::QualifiedName;
use crate::error::Result;
use crate::storage::{Condition, DEFAULT_FETCH_COST, Newest, Row, RowId, Rows, TableStorage};
use crate::txn::{Timestamp, TxId};

/// The partitions holding rows of a partitioned table, by their ids.
pub(crate) struct Dispatched {
    leaves: HashMap<u64, Arc<dyn TableStorage>>,
}

impl Dispatched {
    /// The partitions of `table` in `st`'s tree.
    pub(crate) fn of(st: &State, table: &QualifiedName) -> Dispatched {
        let p = st.catalog.partitioning();
        let mut leaves = HashMap::new();
        for leaf in p.leaves(table) {
            if let (Some(of), Ok(storage)) = (p.of(&leaf), st.storage_of(&leaf)) {
                leaves.insert(of.id, Arc::clone(storage));
            }
        }
        Dispatched { leaves }
    }

    /// The storage of the partition `id` names, if in the tree, and the
    /// row's id in it.
    pub(crate) fn leaf(&self, id: RowId) -> Option<(&Arc<dyn TableStorage>, RowId)> {
        let (partition, row) = unpacked(id);
        self.leaves.get(&partition).map(|s| (s, row))
    }
}

/// The error of a write through the partitioned table's rows.
fn written<T>() -> Result<T> {
    Err(api("[DESIGN] a partitioned table's rows are written to its partitions".to_owned()))
}

impl TableStorage for Dispatched {
    fn insert(&self, _tx: TxId, _row: Row) -> Result<RowId> {
        written()
    }
    fn update(&self, _tx: TxId, _id: RowId, _row: Row) -> Result<()> {
        written()
    }
    fn delete(&self, _tx: TxId, _id: RowId) -> Result<()> {
        written()
    }
    fn restore(&self, _tx: TxId, _id: RowId, _row: Row) -> Result<()> {
        written()
    }
    fn get(&self, tx: TxId, id: RowId) -> Result<Option<Row>> {
        self.leaf(id).map_or(Ok(None), |(s, row)| s.get(tx, row))
    }
    fn get_columns(&self, tx: TxId, id: RowId, columns: &[usize]) -> Result<Option<Row>> {
        self.leaf(id).map_or(Ok(None), |(s, row)| s.get_columns(tx, row, columns))
    }
    /// None, as its rows are its partitions', each read through them.
    fn scan(self: Arc<Self>, _tx: TxId, _columns: &[usize]) -> Rows {
        Box::new(std::iter::empty())
    }
    fn row_count(&self) -> usize {
        self.leaves.values().map(|s| s.row_count()).fold(0, usize::saturating_add)
    }
    /// Told to each partition, as any of them may hold the rows read.
    fn read_by_condition(&self, tx: TxId, condition: &Condition) -> Result<()> {
        self.leaves.values().try_for_each(|s| s.read_by_condition(tx, condition))
    }
    fn newest(&self, tx: TxId, id: RowId) -> Result<Newest> {
        self.leaf(id).map_or_else(|| Ok(Newest::of(None)), |(s, row)| s.newest(tx, row))
    }
    fn newest_writer(&self, tx: TxId, id: RowId) -> Result<(Option<TxId>, Timestamp)> {
        self.leaf(id).map_or(Ok((None, 0)), |(s, row)| s.newest_writer(tx, row))
    }
    fn fetch_cost(&self) -> f64 {
        self.leaves.values().next().map_or(DEFAULT_FETCH_COST, |s| s.fetch_cost())
    }
}

// Unit tests: rows read from the partition their id names, writes refused.
#[cfg(test)]
mod tests;
