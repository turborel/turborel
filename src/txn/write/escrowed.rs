//! \[DESIGN\] Row writes and the reservations of increments (RULES.md
//! D452): a write waits for the transactions whose reservations guard a
//! column it changes, before it and once it is made.

use super::Write;
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::db::write::system_time::Stamp;
use crate::error::Result;
use crate::storage::{Row, RowId};
use crate::txn::{TxId, Txn};

/// \[DESIGN\] A write as it finds the reservations of its row (RULES.md
/// D452).
pub(super) enum Guarded<T> {
    /// None it must wait for: what it did.
    Written(T),
    /// It waited for one: it looks again.
    Waited,
}

impl State {
    /// \[DESIGN\] `write` of row `id` by `tx`, made by `f`, unless another
    /// transaction's reservation of the row keeps it from being made: then
    /// that transaction waited for, its `turn` taken, so it looks again
    /// (RULES.md D452). A write that applies increments (`fold`) is made at
    /// once. The look and the write exclude a reservation of the row, so a
    /// write never waits for a reservation while it holds the row.
    pub(super) fn unreserved<T, F: FnOnce() -> T>(
        &self,
        tx: &Txn,
        (table, id): (&QualifiedName, RowId),
        (write, fold): (&Write, bool),
        (turn, f): (&mut Option<crate::txn::row_waits::Turn>, F),
    ) -> Result<Guarded<T>> {
        if fold {
            return Ok(Guarded::Written(f()));
        }
        let escrow = std::sync::Arc::clone(&self.lock_of(table)?.escrow);
        let found = escrow.exclusive(id, || -> Result<std::result::Result<T, TxId>> {
            Ok(match self.reserved_by(tx, (table, id), write)? {
                Some(by) => Err(by),
                None => Ok(f()),
            })
        })?;
        let by = match found {
            Ok(done) => return Ok(Guarded::Written(done)),
            Err(by) => by,
        };
        drop(turn.take());
        *turn = tx.wait_for_row(&format_args!("a row of {table}"), by)?;
        Ok(Guarded::Waited)
    }

    /// Once a write of row `id` replaced `old`: the version it ended kept
    /// in `history`, if its table keeps one, as `stamp` says. The row
    /// replaced.
    pub(super) fn written(&self, tx: &Txn, (old, history): (Row, Option<QualifiedName>), stamp: Option<&Stamp>) -> Result<Row> {
        if let (Some(history), Some(stamp)) = (history, stamp) {
            self.insert_row(tx, &history, &stamp.ended(&old))?;
        }
        Ok(old)
    }

    /// \[DESIGN\] Another transaction whose increments reserved on row `id`
    /// `write` by `tx` must wait for (RULES.md D452): one guarding a column
    /// the write changes, or any, for a delete.
    fn reserved_by(&self, tx: &Txn, (table, id): (&QualifiedName, RowId), write: &Write) -> Result<Option<TxId>> {
        let lock = self.lock_of(table)?;
        if !lock.escrow.any() {
            return Ok(None);
        }
        let changed = match write {
            Write::Delete => None,
            Write::Update(row) => {
                let newest = self.storage_of(table)?.newest(tx.id, id)?;
                Some(newest.row.map_or(u64::MAX, |old| crate::txn::escrow::changed(&old, row)))
            }
        };
        Ok(lock.escrow.blocking(tx.id, id, changed))
    }
}
