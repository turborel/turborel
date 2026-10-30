//! Transactional operations on the database state: every change to stored
//! rows goes through here, so locking, undo and index maintenance happen in
//! one place (`txn/write.rs` for the writes). Split from txn.rs (RULES.md
//! R-SIZE-2).

use std::sync::Arc;

use super::{Concurrency, Joined, LockMode, Policy, Reading, TxCapabilities, Txn, Undo};
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::error::{Code, Result, SqlError};

impl State {
    pub(crate) fn capabilities(&self, table: &QualifiedName) -> Result<TxCapabilities> {
        Ok(self.storage_of(table)?.capabilities())
    }

    fn check_isolation(&self, table: &QualifiedName, caps: TxCapabilities) -> Result<()> {
        if caps.concurrency == (Concurrency::Engine { serializable: false }) && self.policy() == Policy::Strict {
            return Err(SqlError::new(
                Code::Policy,
                format!(
                    "[X3.135-1986 §4.16] requires serializable execution; the engine of {table} does not provide \
                     it, and the transaction policy is Strict"
                ),
            ));
        }
        Ok(())
    }

    /// The damaged tables, sorted by name.
    pub fn damaged_tables(&self) -> Vec<QualifiedName> {
        // A table dropped is no longer damaged: its lock may stay.
        let mut tables: Vec<QualifiedName> =
            self.locks.iter().filter(|(t, lk)| lk.is_damaged() && self.catalog.table(t).is_some()).map(|(t, _)| t.clone()).collect();
        tables.sort_by_key(ToString::to_string);
        tables
    }

    /// Refuses any use of a damaged table (RULES.md D19).
    fn check_damaged(&self, table: &QualifiedName) -> Result<()> {
        if self.lock_of(table)?.is_damaged() {
            return Err(SqlError::new(Code::Damaged, format!("[DESIGN] {table} is damaged: a change to it could not be undone")));
        }
        Ok(())
    }

    /// Before reading a table: a shared lock, so that the execution is
    /// serializable [X3.135-1986 §4.16].
    pub(crate) fn read_lock(&self, tx: &Txn, table: &QualifiedName) -> Result<()> {
        self.lock_to_read(tx, table, LockMode::Shared)
    }

    /// Before reading a table to write the rows read: an update lock
    /// (`LockMode::Update`).
    pub(crate) fn update_lock(&self, tx: &Txn, table: &QualifiedName) -> Result<()> {
        self.lock_to_read(tx, table, LockMode::Update)
    }

    /// Before reading a table, a lock in `mode` if its engine leaves
    /// concurrency to the coordinator.
    fn lock_to_read(&self, tx: &Txn, table: &QualifiedName, mode: LockMode) -> Result<()> {
        self.check_damaged(table)?;
        self.lock_of(table)?.check_generation(table, self.generation_of(table), tx)?;
        let caps = self.capabilities(table)?;
        self.check_isolation(table, caps)?;
        match caps.concurrency {
            Concurrency::Coordinator => self.acquire(tx, table, mode),
            Concurrency::Engine { .. } => self.join(tx, table),
        }
    }

    /// Makes `table` a participant of `tx`, beginning the transaction in its
    /// engine, unless it is one already. A table joins only once its engine
    /// has begun the transaction, so commit and rollback never reach an
    /// engine whose begin failed. The transaction's bookkeeping
    /// is not locked across the engine's `begin` (RULES.md R-ERR-4).
    fn join(&self, tx: &Txn, table: &QualifiedName) -> Result<()> {
        let joined = Joined { table: table.clone(), storage: Arc::clone(self.storage_of(table)?) };
        if tx.state()?.joined.iter().any(|j| j.is(&joined.storage)) {
            return Ok(());
        }
        joined.storage.begin(tx.id, tx.snapshot())?;
        if tx.reading != Reading::Serializable {
            joined.storage.read_mode(tx.id, tx.read_mode())?;
        }
        tx.state()?.joined.push(joined);
        Ok(())
    }

    /// Before changing a table: policy, lock, participant and savepoint bookkeeping.
    pub(super) fn prepare_write(&self, tx: &Txn, table: &QualifiedName) -> Result<()> {
        self.check_damaged(table)?;
        self.lock_of(table)?.check_generation(table, self.generation_of(table), tx)?;
        let caps = self.capabilities(table)?;
        self.check_isolation(table, caps)?;
        if caps.undo == Undo::Unsupported && self.policy() == Policy::Strict {
            return Err(SqlError::new(
                Code::Policy,
                format!(
                    "[X3.135-1986 §4.16] requires changes to be cancellable; the engine of {table} cannot cancel \
                     them, and the transaction policy is Strict"
                ),
            ));
        }
        self.check_persistence(tx, table, caps.persistence)?;
        tx.state()?.wrote = true;
        let storage = self.storage_of(table)?;
        // An engine that isolates transactions itself lets its writers share
        // the table's lock with each other, but not with `create_index`, which
        // must see no uncommitted change (`Database::create_index`).
        let mode = if caps.concurrency == Concurrency::Coordinator { LockMode::Exclusive } else { LockMode::Shared };
        self.acquire(tx, table, mode)?;
        // Holding the lock, the statement may write only if its snapshot
        // knows every index the table has (RULES.md D27).
        if self.lock_of(table)?.changed_after(self.generation) {
            return Err(SqlError::new(
                Code::Conflict,
                format!("[DESIGN] the definitions of {table} changed after the statement began; run it again"),
            ));
        }
        // An index published from here on dooms the transaction instead.
        tx.wrote_under(table, self.lock_of(table)?, self.generation)?;
        if caps.undo == Undo::Native || caps.concurrency != Concurrency::Coordinator {
            self.join(tx, table)?;
        }
        // Only the transaction's own session touches its bookkeeping, so it
        // is the same before and after the engine's `savepoint`.
        let needs_savepoint = caps.undo == Undo::Native && !tx.state()?.statement_savepoints.iter().any(|(j, _)| j.is(storage));
        if needs_savepoint {
            let sp = storage.savepoint(tx.id);
            tx.state()?.statement_savepoints.push((Joined { table: table.clone(), storage: Arc::clone(storage) }, sp));
        }
        Ok(())
    }
}
