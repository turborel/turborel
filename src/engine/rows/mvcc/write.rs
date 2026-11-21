//! A row's change by a transaction through the row store's trait methods
//! (`TableStorage::update`, `delete`, `write_newest`, `Sharing::share`):
//! its version written, its zone widened, and its counts kept.

use std::sync::atomic::Ordering;

use super::Past;
use crate::engine::rows::version::Version;
use crate::engine::rows::{RowTable, no_row};
use crate::error::Result;
use crate::storage::{Row, RowId, Sharing};
use crate::txn::{Timestamp, TxId};

impl Sharing for RowTable {
    fn share(&self, tx: TxId, id: RowId, (row, seen): (Row, Option<Timestamp>), columns: &[usize]) -> Result<(Row, Vec<usize>)> {
        let past = seen.map_or(Past::Snapshot, Past::Stamp);
        let (record, pages) = self.encode(id, &row)?;
        let written = self.write_sharing(tx, id, (Some(&record), past), columns).inspect_err(|_| self.free(&pages))?;
        let Some((replaced, kept)) = written else { return no_row(id) };
        self.widen_over(id, Some(&row), Some(&replaced));
        self.counted_change(tx);
        let old = self.replaced(id, Some(&replaced))?.map_or_else(|| no_row(id), Ok)?;
        Ok((old, kept))
    }
}

impl RowTable {
    /// Row `id` made `row` by `tx`, or deleted with none, its newest
    /// committed version checked as `past` says: the row it replaced,
    /// decoded from the version the write replaced, so the row is not read
    /// before it is written (RULES.md D417); none outside a transaction.
    pub(in crate::engine::rows) fn write_over(&self, tx: TxId, id: RowId, row: Option<Row>, past: Past) -> Result<Option<Row>> {
        let Some(row) = row else {
            let replaced = self.write_version(tx, id, None, past)?;
            self.widen_over(id, None, replaced.as_deref());
            self.counted.live.fetch_sub(1, Ordering::AcqRel);
            self.counted_change(tx);
            return self.replaced(id, replaced.as_deref());
        };
        let (record, pages) = self.encode(id, &row)?;
        let replaced = self.write_version(tx, id, Some(&record), past).inspect_err(|_| self.free(&pages))?;
        self.widen_over(id, Some(&row), replaced.as_deref());
        self.counted_change(tx);
        self.replaced(id, replaced.as_deref())
    }

    /// The row of `replaced`, the version of row `id` a write replaced, if
    /// it found one: its values out of line are freed only as the
    /// transaction ends, so they are still there to read.
    fn replaced(&self, id: RowId, replaced: Option<&[u8]>) -> Result<Option<Row>> {
        replaced.map(|bytes| self.decode(id, Version::parse(bytes)?.record, None)).transpose()
    }
}
