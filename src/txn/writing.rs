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
use crate::storage::{COLUMN_BITS, Row, TableStorage, unchanged};

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
    /// The columns its statement may change, if it said (`setting`):
    /// every other column an update leaves as it was.
    set: Option<Set>,
}

/// \[DESIGN\] The columns a statement said it sets (RULES.md D875), and
/// what an update that sets no other reads of the row it replaces.
pub(super) struct Set {
    /// The columns set, as bits (`storage::column_bit`): those whose
    /// values before an update's log record may hold
    /// (`record::put_update_of`).
    mask: u64,
    /// The columns read of the row replaced, as bits: those set, and the
    /// key columns of each index whose key reads one set, so the key the
    /// row had in such an index can be made of them.
    read: u64,
}

impl Set {
    /// The columns set, in ascending order, each once.
    pub(super) fn columns(&self) -> Columns {
        Columns::of(self.mask)
    }

    /// The columns read of the row replaced, in ascending order.
    pub(super) fn read(&self) -> Columns {
        Columns::of(self.read)
    }
}

/// \[DESIGN\] The columns of a mask, in ascending order, held where they
/// are made (RULES.md D917): each write of a row names the columns it
/// sets and reads, and a list made for each would be an allocation a row.
pub(crate) struct Columns {
    list: [usize; COLUMN_BITS],
    len: usize,
}

impl Columns {
    /// The columns of `mask`, a bit a column (`storage::column_bit`).
    pub(crate) fn of(mask: u64) -> Columns {
        let mut columns = Columns { list: [0; COLUMN_BITS], len: 0 };
        let mut left = mask;
        // Its lowest bit, then the mask without it, until none is left.
        while left != 0 {
            let column = usize::try_from(left.trailing_zeros()).unwrap_or(COLUMN_BITS);
            if let Some(slot) = columns.list.get_mut(columns.len) {
                *slot = column;
            }
            columns.len = columns.len.saturating_add(1);
            left &= left.wrapping_sub(1);
        }
        columns
    }
}

impl std::ops::Deref for Columns {
    type Target = [usize];

    fn deref(&self) -> &[usize] {
        self.list.get(..self.len).unwrap_or_default()
    }
}

/// RULES.md D875: of a statement setting the columns `set`, the columns
/// an update reads of the row it replaces, as bits: those set, with every
/// column of each key of `keys`, an index's key columns as bits, that
/// reads one set. A key reading none set is not read, as its entry is
/// kept with no value compared.
fn read_columns(set: u64, keys: impl Iterator<Item = u64>) -> u64 {
    keys.filter(|key| key & set != 0).fold(set, |read, key| read | key)
}

impl<'s> Writing<'s> {
    /// \[DESIGN\] This, of a statement that changes no column but those
    /// of `set`, a bit a column (`storage::column_bit`). A statement
    /// setting a column past the last bit has no such mask and says
    /// nothing (RULES.md D875). What the statement's rows keep of the
    /// columns it does not set is then known with no row looked at. The
    /// caller answers for `set`: a column a trigger, a generated column or
    /// a period's stamp changes is one the statement changes.
    #[must_use]
    pub(crate) fn setting(mut self, set: u64) -> Writing<'s> {
        let read = read_columns(set, self.indexes.iter().map(|entries| entries.columns));
        self.set = Some(Set { mask: set, read });
        self
    }

    /// \[DESIGN\] RULES.md D875: the columns its statement said it sets,
    /// if an update of its table may read of the row it replaces those of
    /// `Set::read` alone. That is so where the table's engine keeps
    /// versions and undoes its own changes. The coordinator then reads the
    /// row replaced only for its log record, of the columns set, and to
    /// find the index entries the new version shares (`kept_columns`). An
    /// engine the coordinator undoes by its log keeps the whole row to put
    /// back, and one whose index entries the coordinator removes makes
    /// each index's key of it, so each is given the row whole.
    pub(super) fn sparse(&self) -> Option<&Set> {
        let lean = self.caps.undo != Undo::Log && self.caps.concurrency != Concurrency::Coordinator;
        self.set.as_ref().filter(|_| lean)
    }

    /// \[DESIGN\] RULES.md D917: whether an update of its table may be
    /// written as a patch, its row holding the columns set and those given
    /// here, the columns its index keys read, and null in every other. So
    /// it is where the row replaced is read by its columns alone
    /// (`sparse`), no check reads the row written beside its table, no
    /// period stamps it, and each index's key is of columns told here,
    /// which makes the entry of a row whose key it changes.
    pub(crate) fn patched(&self) -> Option<u64> {
        let plain = !self.checked && !self.stamped && self.indexes.iter().all(|entries| entries.columns != 0);
        self.sparse().filter(|_| plain).map(|set| set.mask | self.key_columns)
    }

    /// The columns its statement said it sets, in ascending order.
    pub(super) fn set_columns(&self) -> Option<Columns> {
        self.set.as_ref().map(Set::columns)
    }

    /// RULES.md D443, D875: of `row` written over `old`, the columns the
    /// keys of the table's indexes read that both hold alike. Only those
    /// the statement sets are compared, the others taken as kept.
    pub(super) fn kept_columns(&self, old: &Row, row: &Row) -> u64 {
        let set = self.set.as_ref().map_or(u64::MAX, |set| set.mask);
        unchanged(old, row, self.key_columns & set) | (self.key_columns & !set)
    }

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
        // The table's own indexes, then the global ones over it
        // (`State::table_indexes`), with no list made of them between.
        let (own, globals) = (self.catalog.indexes(table), self.globals_over(table));
        let mut indexes = Vec::with_capacity(own.len().saturating_add(globals.len()));
        for ix in own {
            indexes.push(Entries { columns: key_columns(ix), index: self.indexes.get(&ix.name), ix: Cow::Borrowed(ix), partition: None });
        }
        for (ix, partition) in globals {
            let index = self.indexes.get(&ix.name);
            indexes.push(Entries { ix: Cow::Owned(ix), partition: Some(partition), columns: 0, index });
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
            set: None,
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
