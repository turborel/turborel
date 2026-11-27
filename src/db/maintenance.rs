//! \[DESIGN\] Maintenance (RULES.md D31): the work an engine or an index
//! leaves for later, so no transaction's end, and no plan, waits for it:
//! the row store's gathering of a table's statistics and summing up of its
//! zones; the column store's freezing of rows into groups, its writing of
//! groups again, and its taking out of the entries of rows frozen; an
//! opened index's count of its keys (`Index::maintain`, RULES.md D35).
//!
//! As a transaction's end collects a table's garbage (`txn/end.rs`), it
//! notes the table if its engine has maintenance due
//! (`TableStorage::maintenance_due`), as planning notes one whose
//! statistics, or an index's key counts, it asked for are due
//! (`State::statistics`, `State::distinct_keys`); the session ending it
//! then asks for the database's maintenance job, which the thread manager
//! runs as JOBS.POLICY says (`db/background.rs`, RULES.md D55). The job
//! has each table noted, then each of its indexes, do some of its work
//! (`TableStorage::maintain`, `Index::maintain`), and goes round again at
//! once while any did some; a table still due, though it did none, it
//! looks at again a second later, as a freeze may be waiting out
//! COLUMNSTORE.FREEZE_DELAY. `Database::maintain` does every table's now,
//! until none does more. As the database opens, each table with
//! maintenance due is noted (`State::note_due_maintenance`); the zones of
//! a table opened are due to the job only once one is asked for, so a
//! table nothing reads is not noted for them (RULES.md D1212), while
//! `Database::maintain` sums them up asked or not.
//!
//! The job first collects the garbage of every table awaiting collection
//! (`State::collect_awaiting`, RULES.md D920): a transaction's end
//! collects a table only once a batch of garbage is long due (D925), and one
//! that leaves a table awaiting asks for the job a second later, so the
//! garbage of a table gone quiet waits no longer than that. A run of the
//! job before the second is up, for maintenance noted, makes no pass.
//! `Database::maintain` collects them at once.
//!
//! One table's work is done at a time, under the gate, so a checkpoint
//! waits for it (`db/checkpoint.rs`), and none while the table's
//! collection is held back, or its definition is newer than the state
//! looking at it. A failure leaves the work for a later call, and is
//! told (`Event::Thread`).

use std::sync::Arc;
use std::time::Duration;

use super::background::AGAIN;
use super::{Database, State};
use crate::catalog::QualifiedName;
use crate::error::Result;
use crate::index::Index;
use crate::storage::TableStorage;
use crate::threads::JobPolicy;

impl Database {
    /// \[DESIGN\] Does now the maintenance due of every table, until none
    /// can do more now: see `db/maintenance.rs`. A freeze not yet due, as
    /// COLUMNSTORE.FREEZE_DELAY says, waits still; rows read by a
    /// transaction still open are not frozen until it ends.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database is closed, or this is called from
    /// within maintenance, as by a plug-in calling back; the error of an
    /// engine's maintenance.
    pub fn maintain(&self) -> Result<()> {
        let st = self.snapshot()?;
        let tables: Vec<QualifiedName> = st.catalog.tables().iter().map(|t| t.name.clone()).collect();
        let _turn = self.background().maintenance_turn.take("maintenance")?;
        st.collect_awaiting();
        while st.maintain_round(&tables, true)? {}
        Ok(())
    }

    /// Asks for the maintenance job, if a table has maintenance due; or,
    /// a second from now, if a table awaits collection and the job's pass
    /// has not been asked for since its last (RULES.md D920).
    pub(super) fn maintain_if_due(&self) {
        let Ok(st) = self.snapshot() else { return };
        // Asked first: a run for the maintenance noted makes no pass
        // before its second is up.
        if st.tx.ask_pass() {
            self.request_maintenance_after(AGAIN);
        }
        // [DESIGN] RULES.md D925: at once for a batch due.
        let soon = st.tx.take_pass_soon();
        if soon || st.tx.maintenance_due() {
            self.request_maintenance();
        }
    }
}

/// The maintenance job's work: a round of the tables noted; at once again
/// if one did some, a second later if one is still due.
///
/// # Errors
///
/// SQLCODE -910 if the database is closed, or this is called from within
/// maintenance; the error of an engine's maintenance, the tables still due
/// noted again.
pub(super) fn maintained(db: &Database) -> Result<Option<Duration>> {
    let st = db.snapshot()?;
    let _turn = db.background().maintenance_turn.take("maintenance")?;
    // Collected first: a table collected may be noted for maintenance.
    // The pass waits out its second on the job's thread; a program that
    // runs what is due itself has it made at once (RULES.md D920).
    let wait = if db.job_policy() == JobPolicy::Automatic { AGAIN } else { Duration::ZERO };
    let pass = st.tx.pass_wait(wait);
    if pass == Some(Duration::ZERO) {
        st.collect_awaiting();
    }
    // [DESIGN] RULES.md D1212: a read made maintenance wanted, which no
    // end of a transaction notes, so each table with some due is noted.
    if st.work.maintenance().take() {
        st.note_due_maintenance();
    }
    let tables: Vec<QualifiedName> = st.tx.take_maintenance().into_iter().collect();
    let worked = st.maintain_round(&tables, false)?;
    if worked {
        return Ok(Some(Duration::ZERO));
    }
    // Looked at again once the pass's second is up, or a second later for
    // a table still due, whichever comes first.
    let again = st.tx.maintenance_due().then_some(AGAIN);
    Ok([pass.filter(|due| !due.is_zero()), again].into_iter().flatten().min())
}

impl State {
    /// Notes each table with maintenance due, it or an index of it.
    pub(super) fn note_due_maintenance(&self) {
        for table in self.catalog.tables() {
            let Ok(storage) = self.storage_of(&table.name) else { continue };
            if storage.maintenance_due() || self.indexes_of(&table.name).any(|ix| ix.maintenance_due()) {
                self.tx.note_maintenance(&table.name);
            }
        }
    }

    /// Has each of `tables` do some of its maintenance, `now` if asked to
    /// (`TableStorage::maintain`), noting again those still due: whether
    /// any did some.
    fn maintain_round(&self, tables: &[QualifiedName], now: bool) -> Result<bool> {
        let (mut worked, mut failed) = (false, None);
        for table in tables {
            let Ok(storage) = self.storage_of(table) else { continue };
            match self.maintain_table(table, storage, now) {
                Ok(did) => worked |= did,
                Err(e) => failed = failed.or(Some(e)),
            }
            if storage.maintenance_due() || self.indexes_of(table).any(|ix| ix.maintenance_due()) {
                self.tx.note_maintenance(table);
            }
        }
        failed.map_or(Ok(worked), Err)
    }

    /// Has `table`, of `storage`, and then each of its indexes, do some of
    /// its maintenance, under the gate: whether one did some. An index's
    /// failure leaves the others' work done.
    fn maintain_table(&self, table: &QualifiedName, storage: &Arc<dyn TableStorage>, now: bool) -> Result<bool> {
        let _gate = self.tx.gate.enter();
        let lock = self.lock_of(table)?;
        if lock.collection_held() || lock.changed_after(self.generation) {
            return Ok(false);
        }
        let passes = storage.statistics_passes();
        let mut worked = storage.maintain(self.tx.horizon(), now)?;
        if worked {
            // When it was last compacted (RULES.md D522).
            lock.stamp(self.clock.now(), true);
        }
        // When its statistics were last gathered (RULES.md D991).
        lock.gathered((storage.statistics_passes() != passes).then(|| self.clock.now()));
        let mut failed = None;
        for index in self.indexes_of(table) {
            match index.maintain() {
                Ok(did) => worked |= did,
                Err(e) => failed = failed.or(Some(e)),
            }
        }
        failed.map_or(Ok(worked), Err)
    }

    /// The indexes of `table` the state has.
    fn indexes_of<'a>(&'a self, table: &QualifiedName) -> impl Iterator<Item = &'a Arc<dyn Index>> + 'a {
        let names: Vec<String> = self.catalog.indexes(table).into_iter().map(|ix| ix.name.clone()).collect();
        names.into_iter().filter_map(|name| self.index_of(&name).ok())
    }
}
