//! \[DESIGN\] A row table's rows counted from its zones (RULES.md D351,
//! `TableStorage::count`). The rows of each zone whose rows every reader
//! sees alone (`zones/visible.rs`) taken from the zone, and those of every
//! other zone, and past the last, read in place with no column
//! (`rows/in_place.rs`), so the count is the one a scan of the rows the
//! reader sees makes, at any isolation level. A settled zone of 1,024 rows
//! costs one lock and an addition, not 1,024 rows read.
//!
//! A table that freezes rows, or still holds groups, is counted from its
//! groups' directory instead (`frozen/counts.rs`, RULES.md D1180): a
//! freeze takes out the entries of rows it froze without a change of
//! their zones, so a zone's rows are not the tree's (`rows/frozen.rs`).

use std::iter;
use std::sync::Arc;

use super::RowTable;
use super::mvcc::{EVERY_COLUMN, Read};
use crate::storage::{Counts, Ramp};
use crate::txn::TxId;

impl RowTable {
    /// The counts `TableStorage::count` gives of the rows `tx` sees: of a
    /// table that freezes rows or holds groups, from its directory, else
    /// from its zones. Nothing is read, nor the read noted, until the first
    /// count is asked for, which is when the one way or the other is taken.
    pub(super) fn count_of(self: Arc<Self>, tx: TxId) -> Counts {
        let mut started: Option<Counts> = None;
        Box::new(iter::from_fn(move || {
            started
                .get_or_insert_with(|| {
                    let table = Arc::clone(&self);
                    if table.holds_groups() { table.grouped_counts(tx) } else { table.counts(tx) }
                })
                .next()
        }))
    }

    /// Whether it freezes rows or holds groups, so its zones' rows are not
    /// its rows.
    pub(super) fn holds_groups(&self) -> bool {
        self.frozen.columns().is_some() || !self.frozen.directory().is_ok_and(|directory| directory.groups.is_empty())
    }

    /// The counts of the rows `tx` sees, of the zones settled, then of the
    /// batches of the rest read with no column. The read is noted first,
    /// of the whole table, as a scan's with no condition is (`rows/mvcc.rs`).
    fn counts(self: Arc<Self>, tx: TxId) -> Counts {
        let reader = match self.reader(tx).and_then(|reader| self.note_read(tx, || Read::All(EVERY_COLUMN)).map(|()| reader)) {
            Ok(reader) => reader,
            Err(e) => return Box::new(iter::once(Err(e))),
        };
        let (settled, ranges) = self.zones.settled();
        let read = self.gathered_in((reader, Ramp::whole()), &[], ranges).map(|batch| batch.map(|b| rows_of(b.rows)));
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
