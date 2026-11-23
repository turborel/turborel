//! Transactional operations on the database state: every change to stored
//! rows goes through here, so locking, undo and index maintenance happen in
//! one place (`txn/write.rs` for the writes).

use std::sync::Arc;

use super::locks::acquire_lock;
use super::writing::Writing;
use super::{Concurrency, Joined, LockMode, Policy, Reading, TableLock, TxCapabilities, Txn, Undo};
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
    fn check_damaged(table: &QualifiedName, lock: &TableLock) -> Result<()> {
        if lock.is_damaged() {
            return Err(SqlError::new(Code::Damaged, format!("[DESIGN] {table} is damaged: a change to it could not be undone")));
        }
        Ok(())
    }

    /// Before reading a table: a shared lock, so that the execution is
    /// serializable [X3.135-1986 §4.16].
    pub(crate) fn read_lock(&self, tx: &Txn, table: &QualifiedName) -> Result<()> {
        self.read_lock_of(tx, (table, u64::MAX))
    }

    /// As `read_lock`, of a reader of the columns of `needed` alone, a bit a
    /// column (`LogicalPlan::columns_read`): the increments it reserved on
    /// them applied first, as of any reader of the table (RULES.md D452).
    pub(crate) fn read_lock_of(&self, tx: &Txn, (table, needed): (&QualifiedName, u64)) -> Result<()> {
        self.reserved_read(tx, table, needed)?;
        self.lock_to_read(tx, table, LockMode::Shared)
    }

    /// Before reading a table to write the rows read: an update lock
    /// (`LockMode::Update`).
    pub(crate) fn update_lock(&self, tx: &Txn, table: &QualifiedName) -> Result<()> {
        self.reserved_read(tx, table, u64::MAX)?;
        self.lock_to_read(tx, table, LockMode::Update)
    }

    /// \[DESIGN\] Before `tx` reads the columns of `needed` of `table`: the
    /// increments it reserved on any of them applied, so it reads its own
    /// sums (RULES.md D452).
    fn reserved_read(&self, tx: &Txn, table: &QualifiedName, needed: u64) -> Result<()> {
        let Ok(lock) = self.lock_of(table) else { return Ok(()) };
        if lock.escrow.any() && lock.escrow.unfolded(tx.id, needed) {
            return self.materialize(tx, table);
        }
        Ok(())
    }

    /// Before reading a table, a lock in `mode` if its engine leaves
    /// concurrency to the coordinator; of a partitioned table, of each
    /// partition too.
    fn lock_to_read(&self, tx: &Txn, table: &QualifiedName, mode: LockMode) -> Result<()> {
        // [EXTENSION] a partitioned table's rows are read in its partitions (RULES.md D156).
        for child in self.catalog.partitioning().children(table) {
            self.lock_to_read(tx, &child.table, mode)?;
        }
        let lock = self.lock_of(table)?;
        Self::check_damaged(table, lock)?;
        lock.check_generation(table, self.generation_of(table), tx)?;
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
        let (name, storage) = self.storage_named(table)?;
        if tx.state()?.joined.iter().any(|j| j.is(&storage)) {
            return Ok(());
        }
        let native = storage.capabilities().undo == Undo::Native;
        Self::begin_in(tx, &Joined { table: name, storage }, native)
    }

    /// Begins `tx` in the engine of `joined`, which then joins it; of an
    /// engine that undoes its own changes, `native`, with a savepoint for
    /// each the transaction has.
    fn begin_in(tx: &Txn, joined: &Joined, native: bool) -> Result<()> {
        joined.storage.begin(tx.id, tx.snapshot())?;
        if tx.reading != Reading::Serializable {
            joined.storage.read_mode(tx.id, tx.read_mode())?;
        }
        tx.state()?.joined.push(joined.clone());
        if native {
            tx.joined_after_savepoints(joined);
        }
        Ok(())
    }

    /// Before changing a table: policy, lock, participant and savepoint bookkeeping.
    pub(crate) fn prepare_write(&self, tx: &Txn, table: &QualifiedName) -> Result<()> {
        self.prepare_in(tx, &self.writing(table)?)
    }

    /// As `prepare_write`, of the table of `writing`. What only its lock
    /// and its engine's capabilities decide is judged at every write. What
    /// the transaction does the first time it writes the table, or the
    /// first time in a statement, is done only if its bookkeeping does not
    /// show it done (`Txn::prepared`, RULES.md D872).
    pub(crate) fn prepare_in(&self, tx: &Txn, writing: &Writing<'_>) -> Result<()> {
        let (table, lock, caps) = (writing.table(), writing.lock, writing.caps);
        Self::check_damaged(table, lock)?;
        lock.check_generation(table, writing.generation, tx)?;
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
        if !lock.changed_after(self.generation) && tx.prepared(writing)? {
            return Ok(());
        }
        self.prepare_first(tx, writing)
    }

    /// What a write of the table of `writing` does in `tx` the first time,
    /// and the first time in a statement: the policy's check of its
    /// persistence, its lock, its joining of the engine, and the
    /// statement's savepoint there.
    fn prepare_first(&self, tx: &Txn, writing: &Writing<'_>) -> Result<()> {
        let (table, lock, caps, storage) = (writing.table(), writing.lock, writing.caps, writing.storage);
        self.check_persistence(tx, table, caps.persistence)?;
        tx.state()?.wrote = true;
        // An engine that isolates transactions itself lets its writers share
        // the table's lock with each other, but not with `create_index`, which
        // must see no uncommitted change (`Database::create_index`).
        let mode = if caps.concurrency == Concurrency::Coordinator { LockMode::Exclusive } else { LockMode::Shared };
        acquire_lock(tx, table, lock, mode)?;
        // Holding the lock, the statement may write only if its snapshot
        // knows every index the table has (RULES.md D27).
        if lock.changed_after(self.generation) {
            return Err(SqlError::new(
                Code::Conflict,
                format!("[DESIGN] the definitions of {table} changed after the statement began; run it again"),
            ));
        }
        // An index published from here on dooms the transaction instead.
        tx.wrote_under(table, lock, self.generation)?;
        let native = caps.undo == Undo::Native;
        if writing.joins() && !tx.state()?.joined.iter().any(|j| j.is(storage)) {
            Self::begin_in(tx, &Joined { table: Arc::clone(writing.shared_name()), storage: Arc::clone(storage) }, native)?;
        }
        // Only the transaction's own session touches its bookkeeping, so it
        // is the same before and after the engine's `savepoint`.
        let needs_savepoint = native && !tx.state()?.statement_savepoints.iter().any(|(j, _)| j.is(storage));
        if needs_savepoint {
            let sp = storage.savepoint(tx.id);
            tx.state()?.statement_savepoints.push((Joined { table: Arc::clone(writing.shared_name()), storage: Arc::clone(storage) }, sp));
        }
        Ok(())
    }
}
