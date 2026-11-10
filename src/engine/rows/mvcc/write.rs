//! A row's change by a transaction through the row store's trait methods
//! (`TableStorage::update`, `delete`, `write_newest`): its version written,
//! its zone widened, and its counts kept.

use std::sync::atomic::Ordering;

use super::Past;
use crate::engine::rows::RowTable;
use crate::error::Result;
use crate::storage::{Row, RowId};
use crate::txn::TxId;

impl RowTable {
    /// Row `id` made `row` by `tx`, or deleted with none, its newest
    /// committed version checked as `past` says.
    pub(in crate::engine::rows) fn write_over(&self, tx: TxId, id: RowId, row: Option<Row>, past: Past) -> Result<()> {
        let Some(row) = row else {
            let replaced = self.write_version(tx, id, None, past)?;
            self.widen_over(id, None, replaced.as_deref());
            self.counted.live.fetch_sub(1, Ordering::AcqRel);
            self.counted_change(tx);
            return Ok(());
        };
        let (record, pages) = self.encode(id, &row)?;
        let replaced = self.write_version(tx, id, Some(&record), past).inspect_err(|_| self.free(&pages))?;
        self.widen_over(id, Some(&row), replaced.as_deref());
        self.counted_change(tx);
        Ok(())
    }
}
