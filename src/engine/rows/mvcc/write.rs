//! A row's change by a transaction through the row store's trait methods
//! (`TableStorage::update`, `delete`, `write_newest`): its version written,
//! its zone widened, and its counts kept; and the columns it kept, whose
//! index entries its version shares with the one it replaced (RULES.md
//! D443).

use std::sync::atomic::Ordering;

use super::Past;
use crate::engine::rows::RowTable;
use crate::engine::rows::version::Version;
use crate::error::Result;
use crate::storage::{Row, RowId};
use crate::sync::lock_builtin;
use crate::txn::{NO_TRANSACTION, TxId};

impl RowTable {
    /// Row `id` made `row` by `tx`, or deleted with none, its newest
    /// committed version checked as `past` says: the row it replaced,
    /// decoded from the version the write replaced, so the row is not read
    /// before it is written (RULES.md D417); none outside a transaction.
    pub(in crate::engine::rows) fn write_over(&self, tx: TxId, id: RowId, row: Option<&Row>, past: Past) -> Result<Option<Row>> {
        let Some(row) = row else {
            let replaced = self.write_version(tx, id, None, past)?;
            let old = self.replaced(id, replaced.as_deref());
            self.widen_over(id, None, live(&old));
            self.counted.live.fetch_sub(1, Ordering::AcqRel);
            self.counted_change(tx);
            return old.map(|old| old.map(|(was, _)| was));
        };
        let (mut framed, pages) = self.encode_framed(id, row)?;
        let replaced = self.write_version(tx, id, Some(&mut framed), past).inspect_err(|_| self.free(&pages))?;
        let old = self.replaced(id, replaced.as_deref());
        self.widen_over(id, Some(row), live(&old));
        self.counted_change(tx);
        old.map(|old| old.map(|(was, _)| was))
    }

    /// As `write_over`, for a caller that does not read the row replaced.
    /// Of that row only the columns its zone keeps are decoded, none of a
    /// table whose zones keep none (`widen_past`, RULES.md D835).
    pub(in crate::engine::rows) fn write_unread(&self, tx: TxId, id: RowId, row: Option<&Row>, past: Past) -> Result<()> {
        let replaced = match row {
            Some(row) => {
                let (mut framed, pages) = self.encode_framed(id, row)?;
                self.write_version(tx, id, Some(&mut framed), past).inspect_err(|_| self.free(&pages))?
            }
            None => self.write_version(tx, id, None, past)?,
        };
        let widened = self.widen_past(id, row, replaced.as_deref());
        if row.is_none() {
            self.counted.live.fetch_sub(1, Ordering::AcqRel);
        }
        self.counted_change(tx);
        widened
    }

    /// Notes `unchanged` on `tx`'s last change, if it replaced a version of
    /// row `id`; the columns noted: none for a change already spilled, or
    /// another's, whose entries are then each version's own
    /// (`TableStorage::keep_entries`).
    pub(in crate::engine::rows) fn keep_columns(&self, tx: TxId, id: RowId, unchanged: u64) -> u64 {
        if tx == NO_TRANSACTION || unchanged == 0 {
            return 0;
        }
        let Ok(open) = self.open_of(tx) else { return 0 };
        let mut open = lock_builtin(&open);
        let replaced = |c: &&mut super::Change| c.id == id && !c.made && (c.before.is_some() || c.appended.is_some());
        let Some(change) = open.changes.last_mut().filter(replaced) else { return 0 };
        change.unchanged = unchanged;
        unchanged
    }

    /// The row of `replaced`, the version of row `id` a write replaced, if
    /// it found one, and whether that version is a deletion: its values
    /// out of line are freed only as the transaction ends, so they are
    /// still there to read. It is decoded once, for the write's caller and
    /// its zone alike (RULES.md D835).
    fn replaced(&self, id: RowId, replaced: Option<&[u8]>) -> Result<Option<(Row, bool)>> {
        let decoded = |bytes| {
            let version = Version::parse(bytes)?;
            Ok((self.decode(id, version.record, None)?, version.deleted))
        };
        replaced.map(decoded).transpose()
    }
}

/// Of the row a write replaced, as `RowTable::replaced` decoded it, the
/// values its zone is widened over: none of a deletion, or of a version
/// not read.
fn live(old: &Result<Option<(Row, bool)>>) -> Option<&Row> {
    old.as_ref().ok()?.as_ref().filter(|(_, deleted)| !deleted).map(|(row, _)| row)
}
