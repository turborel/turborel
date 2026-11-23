//! \[DESIGN\] What a statement's writes of one table resolve once (RULES.md
//! D872). A row written needs its table's storage and what it can do, its
//! lock, the generation and the seal its changes are logged with, and its
//! indexes with the columns their keys read. Each was looked up by the
//! table's name for every row, some several times. A `Writing` holds them,
//! found once in the definitions its statement reads, and each write of a
//! row takes it (`insert_row_in`, `rewrite_row_in` and the others of
//! `txn/write.rs`). The entry points that take a table's name make one for
//! their row, so a caller that writes one row pays what it paid.
//!
//! What a write does in its transaction the first time, its policy checks,
//! its table's lock, its joining of the engine and its statement's
//! savepoint, is found done in the transaction's own bookkeeping, under
//! its lock taken once (`Txn::prepared`), so it is never taken as done
//! past a statement's end, a mark undone or a savepoint gone back to,
//! which each take it back there.

use std::borrow::Cow;
use std::sync::Arc;

use super::kept::key_columns;
use super::{Concurrency, Persistence, TableLock, TxCapabilities, Txn, Undo};
use crate::catalog::{IndexDesc, QualifiedName};
use crate::db::State;
use crate::disk::encoding::Pipeline;
use crate::error::{Code, Result, SqlError};
use crate::index::Index;
use crate::storage::TableStorage;

/// One index a table's rows have entries in, as a statement's writes find
/// it: the table's own, or \[EXTENSION\] a global index over a partition,
/// whose entries carry the partition's id (RULES.md D157).
pub(crate) struct Entries<'s> {
    /// Its description.
    pub(crate) ix: Cow<'s, IndexDesc>,
    /// The partition's id its entries are packed with, of a global index.
    pub(crate) partition: Option<u64>,
    /// The columns its key reads, as bits, which an update that keeps them
    /// shares its entry by (RULES.md D443); none of a global index, or of
    /// one reading a column past the last bit.
    pub(super) columns: u64,
    /// The index, if the definitions hold it.
    index: Option<&'s Arc<dyn Index>>,
}

impl<'s> Entries<'s> {
    /// The entries of `ix` alone, in `index` if the definitions hold it:
    /// of a caller that adds or removes one index's entries by its
    /// description, each under the id it gives.
    pub(super) fn of(ix: &'s IndexDesc, index: Option<&'s Arc<dyn Index>>) -> Entries<'s> {
        Entries { ix: Cow::Borrowed(ix), partition: None, columns: 0, index }
    }

    /// The index.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the definitions do not hold it.
    pub(super) fn index(&self) -> Result<&Arc<dyn Index>> {
        self.index.ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] the index {} does not exist", self.ix.name)))
    }
}

/// \[DESIGN\] What the writes of one table resolve once, for as long as
/// the definitions they were found in are read (RULES.md D872).
pub(crate) struct Writing<'s> {
    name: &'s Arc<QualifiedName>,
    pub(super) storage: &'s Arc<dyn TableStorage>,
    pub(super) lock: &'s Arc<TableLock>,
    pub(super) caps: TxCapabilities,
    /// The generation of its storage, which its row changes are logged with.
    pub(super) generation: u32,
    /// The pipeline its rows are sealed through, if it names encodings.
    pub(super) seal: Option<Arc<Pipeline>>,
    pub(super) indexes: Vec<Entries<'s>>,
    /// The columns the keys of its own indexes read, together.
    pub(super) key_columns: u64,
    /// Whether a row written is checked against what the catalog registers
    /// beside the table: a spatial column's system, a partition's bounds.
    pub(super) checked: bool,
    /// Whether it has a system-time period, whose columns a write stamps.
    pub(super) stamped: bool,
}

impl Writing<'_> {
    /// The table's name.
    pub(crate) fn table(&self) -> &QualifiedName {
        self.name
    }

    /// The table's name, as the definitions share it.
    pub(super) fn shared_name(&self) -> &Arc<QualifiedName> {
        self.name
    }

    /// Whether a write joins the table's engine: one that undoes its own
    /// changes, or isolates its transactions itself.
    pub(super) fn joins(&self) -> bool {
        self.caps.undo == Undo::Native || self.caps.concurrency != Concurrency::Coordinator
    }
}

impl State {
    /// What writes of `table` need of these definitions, found once.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the table has no storage or no lock, or an encoding
    /// it names is not registered.
    pub(crate) fn writing(&self, table: &QualifiedName) -> Result<Writing<'_>> {
        let found = self.storage.get_key_value(table);
        let (name, storage) = found.ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] {table} has no storage")))?;
        let lock = self.lock_of(table)?;
        let desc = self.catalog.table(table);
        let seal = desc.map(|desc| self.seal_of_desc(desc)).transpose()?.flatten();
        let mut indexes = Vec::new();
        for (ix, partition) in self.table_indexes(table) {
            let columns = if partition.is_none() { key_columns(&ix) } else { 0 };
            let index = self.indexes.get(&ix.name);
            indexes.push(Entries { ix, partition, columns, index });
        }
        Ok(Writing {
            name,
            storage,
            lock,
            caps: storage.capabilities(),
            generation: desc.map_or(0, |desc| desc.generation),
            seal,
            key_columns: indexes.iter().fold(0, |all, entries| all | entries.columns),
            indexes,
            checked: !self.catalog.spatial().columns().is_empty() || !self.catalog.partitioning().is_empty(),
            stamped: desc.is_some_and(|desc| desc.period_columns(crate::ast::SYSTEM_TIME).is_some()),
        })
    }
}

impl Txn {
    /// \[DESIGN\] Whether it has done already, and still holds, all a
    /// write through `writing` does the first time (`State::prepare_in`):
    /// it wrote the table under its lock, no table it wrote has had its
    /// definitions changed since, it has joined the table's engine if a
    /// write joins it, and its statement has the engine's savepoint if the
    /// engine undoes its own changes. Read from its own bookkeeping, under
    /// its lock taken once, so nothing is remembered apart from it. Only
    /// of a table whose changes the log holds, first written as such: the
    /// policy's refusals of any other are judged at every write.
    ///
    /// MC/DC pairs, in `writing::tests`: written or not, joined or not, the
    /// statement's savepoint taken or not, a table's definitions changed or
    /// not, each alone deciding.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if its state's lock is poisoned.
    pub(super) fn prepared(&self, writing: &Writing<'_>) -> Result<bool> {
        if writing.caps.persistence != Persistence::Logged {
            return Ok(false);
        }
        let st = self.state()?;
        let is = |joined: &super::Joined| joined.is(writing.storage);
        let written =
            st.wrote && st.first_write_external == Some(false) && st.written.iter().any(|(_, lock, _)| Arc::ptr_eq(lock, writing.lock));
        let unchanged = !st.written.iter().any(|(_, lock, generation)| lock.changed_after(*generation));
        let joined = !writing.joins() || st.joined.iter().any(is);
        let saved = writing.caps.undo != Undo::Native || st.statement_savepoints.iter().any(|(engine, _)| is(engine));
        Ok(written && unchanged && joined && saved)
    }
}

// Unit tests: what a write finds done, and what takes it back (RULES.md
// R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests;
