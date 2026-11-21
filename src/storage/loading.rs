//! \[EXTENSION\] A storage's side of a load (RULES.md D533): a column
//! store table, empty of rows not yet frozen as a load into it begins,
//! under its lock held alone, takes the load's batches straight as frozen
//! groups, seen by the loading transaction alone until it commits, and by
//! each snapshot taken after.

use crate::error::Result;
use crate::storage::{Row, RowId};
use crate::txn::TxId;

/// What a storage that writes a load's rows straight as groups does.
pub trait Loading {
    /// Begins writing the rows `tx` loads straight as groups, if it may:
    /// how many rows a group takes, none if it may not.
    ///
    /// # Errors
    ///
    /// Reading its tree fails.
    fn begin_load(&self, tx: TxId) -> Result<Option<usize>>;

    /// Writes `rows`, of `tx`, as a group seen by `tx` alone until it
    /// commits, undone as its other changes are: their ids, in order; none
    /// if its load no longer goes straight to groups, a row of it having
    /// gone to the tree. Its columns are written on up to `threads`
    /// threads at once.
    ///
    /// # Errors
    ///
    /// Writing the group fails.
    fn insert_group(&self, tx: TxId, rows: &[Row], threads: usize) -> Result<Option<Vec<RowId>>>;

    /// Ends the load of `tx`, if one is under way: rows it inserts from
    /// now on go to the tree.
    fn end_load(&self, tx: TxId);

    /// Ends recovery's part in loads: the groups of a transaction a crash
    /// left open as the table was last stored are dropped if `redone`
    /// says recovery redid or undid its rows, kept as committed otherwise.
    ///
    /// # Errors
    ///
    /// Freeing a group or storing the directory fails.
    fn recovered_loads(&self, redone: &dyn Fn(TxId) -> bool) -> Result<()>;
}
