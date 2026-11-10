//! \[DESIGN\] The newest state of a row (`TableStorage::newest`).

use super::Row;
use crate::txn::{Timestamp, TxId};

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
    /// The open transaction whose change is `pending`, if one is: the one
    /// a writer of the row waits for (RULES.md D28).
    pub writer: Option<TxId>,
    /// The commit timestamp of `row`'s version, which `write_newest` is
    /// given back; 0 for an engine that keeps no versions, an absent row,
    /// or the transaction's own change.
    pub stamp: Timestamp,
}

impl Newest {
    /// `row`, as an engine keeping no versions has it: no change pending.
    pub fn of(row: Option<Row>) -> Newest {
        Newest { row, pending: None, writer: None, stamp: 0 }
    }
}
