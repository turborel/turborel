//! \[DESIGN\] Maintenance (RULES.md D31): the work an engine leaves for
//! later, so no transaction's end waits for it: the column store's freezing
//! of rows into groups, its writing of groups again, and its taking out of
//! the entries of rows frozen.
//!
//! As a transaction's end collects a table's garbage (`txn/end.rs`), it
//! notes the table if its engine has maintenance due
//! (`TableStorage::maintenance_due`); the session ending it then asks the
//! database's maintenance thread, started the first time. The thread has
//! each table noted do some of its work (`TableStorage::maintain`), and
//! goes round again at once while any did some; a table still due, though
//! it did none, it looks at again a second later, as a freeze may be
//! waiting out COLUMNSTORE.FREEZE_DELAY. `Database::maintain` does every
//! table's now, until none does more.
//!
//! One table's work is done at a time, under the gate, so a checkpoint
//! waits for it (`db/checkpoint.rs`), and none while the table's
//! collection is held back, or its definition is newer than the state
//! looking at it. A failure leaves the work for a later call.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::background::Worker;
use super::{Database, State};
use crate::catalog::QualifiedName;
use crate::error::Result;
use crate::storage::TableStorage;
use crate::sync::lock;

/// \[DESIGN\] How long the maintenance thread waits before it looks again
/// at a table still due that did no work: a second.
const AGAIN: Duration = Duration::from_secs(1);

/// \[DESIGN\] A database's maintenance thread, and the turn its work takes.
#[derive(Default)]
pub(super) struct Maintenance {
    worker: Worker,
    /// Held while tables are maintained, so the thread and
    /// `Database::maintain` take turns.
    turn: Mutex<()>,
}

impl Database {
    /// \[DESIGN\] Does now the maintenance due of every table, until none
    /// can do more now: see `db/maintenance.rs`. A freeze not yet due, as
    /// COLUMNSTORE.FREEZE_DELAY says, waits still; rows read by a
    /// transaction still open are not frozen until it ends.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database is closed; the error of an engine's
    /// maintenance.
    pub fn maintain(&self) -> Result<()> {
        let st = self.snapshot()?;
        let tables: Vec<QualifiedName> = st.catalog.tables().iter().map(|t| t.name.clone()).collect();
        let _turn = lock(&self.maintenance().turn)?;
        while st.maintain_round(&tables)? {}
        Ok(())
    }

    /// Asks the maintenance thread, started if need be, for its work, if
    /// a table has maintenance due.
    pub(super) fn maintain_if_due(&self) {
        let Ok(st) = self.snapshot() else { return };
        let worker = &self.maintenance().worker;
        if st.tx.maintenance_due() && worker.start("turborel-maintenance", self.downgrade(), maintained).is_ok() {
            worker.request();
        }
    }

    /// Ends the maintenance thread, after the work it is doing.
    pub(super) fn stop_maintenance(&self) {
        self.maintenance().worker.stop();
    }
}

/// The maintenance thread's work: a round of the tables noted; at once
/// again if one did some, a second later if one is still due.
fn maintained(db: &Database) -> Option<Duration> {
    let st = db.snapshot().ok()?;
    let tables: Vec<QualifiedName> = st.tx.take_maintenance().into_iter().collect();
    let worked = {
        let _turn = lock(&db.maintenance().turn).ok()?;
        st.maintain_round(&tables)
    };
    match worked {
        Ok(true) => Some(Duration::ZERO),
        Ok(false) | Err(_) => st.tx.maintenance_due().then_some(AGAIN),
    }
}

impl State {
    /// Has each of `tables` do some of its maintenance, noting again
    /// those still due: whether any did some.
    fn maintain_round(&self, tables: &[QualifiedName]) -> Result<bool> {
        let (mut worked, mut failed) = (false, None);
        for table in tables {
            let Ok(storage) = self.storage_of(table) else { continue };
            match self.maintain_table(table, storage) {
                Ok(did) => worked |= did,
                Err(e) => failed = failed.or(Some(e)),
            }
            if storage.maintenance_due() {
                self.tx.note_maintenance(table);
            }
        }
        failed.map_or(Ok(worked), Err)
    }

    /// Has `table`, of `storage`, do some of its maintenance, under the
    /// gate: whether it did some.
    fn maintain_table(&self, table: &QualifiedName, storage: &Arc<dyn TableStorage>) -> Result<bool> {
        let _gate = self.tx.gate.enter();
        let lock = self.lock_of(table)?;
        if lock.collection_held() || lock.changed_after(self.generation) {
            return Ok(false);
        }
        storage.maintain(self.tx.horizon())
    }
}
