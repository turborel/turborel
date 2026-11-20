//! \[DESIGN\] A row table's rows counted from its zones (RULES.md D351,
//! `TableStorage::count`): the rows of each zone whose rows every reader
//! sees alone (`zones/visible.rs`) taken from the zone, and those of every
//! other zone, and past the last, read in place with no column
//! (`rows/in_place.rs`), so the count is the one a scan of the rows the
//! reader sees makes, at any isolation level. A settled zone of 1,024 rows
//! costs one lock and an addition, not 1,024 rows read.
//!
//! A table that freezes rows, or still holds groups, is not counted so:
//! a freeze takes out the entries of rows it froze without a change of
//! their zones, so a zone's rows are not the tree's (`rows/frozen.rs`).

use std::iter;
use std::sync::Arc;

use super::RowTable;
use super::mvcc::Read;
use crate::storage::Counts;
use crate::txn::TxId;

impl RowTable {
    /// The counts `TableStorage::count` gives of the rows `tx` sees: none
    /// of a table that freezes rows or holds groups. Nothing is read, nor
    /// the read noted, until the first count is asked for.
    pub(super) fn count_of(self: Arc<Self>, tx: TxId) -> Option<Counts> {
        let freezes = self.frozen.columns().is_some();
        if freezes || !self.frozen.directory().is_ok_and(|directory| directory.groups.is_empty()) {
            return None;
        }
        let mut started: Option<Counts> = None;
        Some(Box::new(iter::from_fn(move || started.get_or_insert_with(|| Arc::clone(&self).counts(tx)).next())))
    }

    /// The counts of the rows `tx` sees: of the zones settled, then of the
    /// batches of the rest read with no column. The read noted first, of
    /// the whole table, as a scan's with no condition is (`rows/mvcc.rs`).
    fn counts(self: Arc<Self>, tx: TxId) -> Counts {
        let reader = match self.reader(tx).and_then(|reader| self.note_read(tx, || Read::All).map(|()| reader)) {
            Ok(reader) => reader,
            Err(e) => return Box::new(iter::once(Err(e))),
        };
        let (settled, ranges) = self.zones.settled();
        let read = self.gathered_in(reader, &[], ranges).map(|batch| batch.map(|b| rows_of(b.rows)));
        Box::new(iter::once(Ok(settled)).chain(read))
    }
}

/// `rows`, a batch's, as a count.
fn rows_of(rows: usize) -> u64 {
    u64::try_from(rows).unwrap_or(u64::MAX)
}


// Unit tests: the count is the scan's for every reader, of zones settled
// and not (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests;
