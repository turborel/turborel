//! \[EXTENSION\] A storage's side of a load (RULES.md D533): a column
//! store table, empty of rows not yet frozen as a load into it begins,
//! under its lock held alone, takes the load's batches straight as frozen
//! groups, seen by the loading transaction alone until it commits, and by
//! each snapshot taken after.

use crate::error::Result;
use crate::memory::Reservation;
use crate::storage::{Row, RowId};
use crate::txn::TxId;

/// \[DESIGN\] A group a load wrote (RULES.md D1146): its rows' ids, in
/// order, and the extent it was written as, if it went to a segment, for
/// its log record, which is then the load's record of its rows.
pub struct Loaded {
    /// Its rows' ids, in order.
    pub ids: Vec<RowId>,
    /// Its extent, as its segment holds it.
    pub extent: Option<Vec<u8>>,
    /// The charge of the extent to the memory budget, held until it is
    /// logged (RULES.md D1149).
    pub held: Option<Reservation>,
}

/// What a storage that writes a load's rows straight as groups does.
pub trait Loading {
    /// Begins writing the rows `tx` loads straight as groups, if it may:
    /// how many rows a group takes, none if it may not. `qualifies` says
    /// whether its table qualifies for extents (no index, constraint but
    /// NOT NULL, trigger or large object), so a load of no place asked
    /// goes to groups by default (RULES.md D1147).
    ///
    /// # Errors
    ///
    /// Reading its tree fails.
    fn begin_load(&self, tx: TxId, qualifies: bool) -> Result<Option<usize>>;

    /// Writes `rows`, of `tx`, as a group seen by `tx` alone until it
    /// commits, undone as its other changes are: their ids, in order, and
    /// its extent if it went to a segment, as it may if `whole`; none if
    /// its load no longer goes straight to groups, a row of it having gone
    /// to the tree.
    ///
    /// # Errors
    ///
    /// Writing the group fails.
    fn insert_group(&self, tx: TxId, rows: &[Row], whole: bool) -> Result<Option<Loaded>>;

    /// Redoes the group of a committed load whose log record holds
    /// `extent`, of rows from `first`, unless a group of its range is there
    /// (RULES.md D1146).
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if the extent is not whole; writing it fails.
    fn redo_group(&self, first: RowId, extent: &[u8]) -> Result<()>;

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
