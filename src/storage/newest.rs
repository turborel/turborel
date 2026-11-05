//! \[DESIGN\] The newest state of a row (`TableStorage::newest`).

use super::Row;

/// \[DESIGN\] The newest state of a row, for checking UNIQUE against rows a
/// transaction may not see (`TableStorage::newest`).
#[derive(Debug, Clone)]
pub struct Newest {
    /// The newest committed version, or the transaction's own change: none
    /// if the row is absent or deleted.
    pub row: Option<Row>,
    /// A change another open transaction made to it: the row it leaves,
    /// none for a delete. While it is pending the row is in doubt.
    pub pending: Option<Option<Row>>,
}
