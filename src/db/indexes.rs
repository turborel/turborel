//! \[EXTENSION\] CREATE INDEX on a table that transactions go on writing
//! (RULES.md D27). Split from db/database.rs (RULES.md R-SIZE-2).
//!
//! An index holds an entry for every version of a row that has its key,
//! which garbage collection takes back when it removes the version
//! (`Index`). Building one while transactions write the table, without
//! stopping them:
//!
//! 1. **Published** with the gate shut, so no write or commit is half done:
//!    in the catalog, being built. Every write from then on keeps it up to
//!    date, under its transaction's undo, but no query reads it. Its table's
//!    definitions are marked changed: a transaction that wrote the table
//!    before can then neither commit nor go on, and is refused with SQLCODE
//!    -911, to run again (`Txn::check_definitions`). The build takes its
//!    snapshot at that moment.
//! 2. **Filled** with an entry for every version committed by that snapshot
//!    that a reader may read or garbage collection will report
//!    (`TableStorage::versions`); later versions are their writers'. The
//!    table's garbage is not collected meanwhile, and no checkpoint is
//!    taken.
//! 3. **Published ready** to read, and logged. If filling fails, it is
//!    taken out of the catalog instead; its structure stays, for the undo
//!    of the writes that kept it up to date.
//!
//! A table whose engine leaves concurrency to the coordinator keeps no
//! versions: its index is built under the table's exclusive lock.

use std::sync::Arc;

use super::{Database, State, api, qualified_name};
use crate::catalog::{IndexDesc, QualifiedName};
use crate::error::{Code, Result, SqlError, fail};
use crate::index::{BTREE_METHOD, HASH_METHOD, Index};
use crate::log::record::Change;
use crate::storage::{TableOption, chosen, needs_rebuild};
use crate::txn::{Concurrency, LockMode, TableLock, Timestamp, Transactions, Txn};
use crate::value::Value;

/// \[EXTENSION\] An index to make: its name, its table, its columns by
/// name, its method, and the method's options it sets (RULES.md D41).
#[derive(Debug, Clone)]
pub(crate) struct IndexSpec {
    pub(crate) name: String,
    pub(crate) table: QualifiedName,
    pub(crate) columns: Vec<String>,
    pub(crate) method: String,
    pub(crate) options: Vec<(String, Value)>,
}

impl IndexSpec {
    /// The definition the log keeps of it.
    pub(super) fn change(&self) -> Change {
        let IndexSpec { name, table, columns, method, options } = self.clone();
        Change::CreateIndex { name, table, columns, method, options }
    }
}

/// An index build under way, counted where it matters until dropped: by its
/// table, whose garbage waits, and by the transactions, whose checkpoints
/// wait.
struct Building {
    lock: Arc<TableLock>,
    transactions: Arc<Transactions>,
}

impl Building {
    fn begin(st: &State, table: &QualifiedName) -> Result<Building> {
        let lock = Arc::clone(st.lock_of(table)?);
        lock.hold_collection();
        st.tx.begin_build();
        Ok(Building { lock, transactions: Arc::clone(&st.tx) })
    }
}

impl Drop for Building {
    fn drop(&mut self) {
        self.lock.release_collection();
        self.transactions.end_build();
    }
}

impl State {
    /// The options an index of `method` takes: the method's, then those
    /// plug-ins declared for every index (`Database::declare_index_option`).
    pub(crate) fn index_options_of(&self, method: &str) -> Vec<TableOption> {
        let own = self.methods.get(method).map(|m| m.index_options()).unwrap_or_default();
        own.into_iter().chain(self.index_options.iter().map(|(_, o)| o.clone())).collect()
    }

    /// Index `name` built again, `built`, put in its place: under its name,
    /// and enforcing what it enforced; its old structure discarded, read
    /// still by earlier snapshots.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if either is not there: the old one dropped meanwhile.
    fn swap_index(&mut self, (built, name): (&str, &str)) -> Result<()> {
        let old = self.catalog.index(name).cloned().ok_or_else(|| api(format!("[EXTENSION] index {name} was dropped")))?;
        let new = self.catalog.index(built).cloned().ok_or_else(|| api(format!("[EXTENSION] no index {built}")))?;
        let mut catalog = self.catalog.duplicate();
        catalog.remove_index(built);
        catalog.remove_index(name);
        catalog.add_index(IndexDesc { name: name.to_owned(), unique: old.unique, ..new });
        self.catalog = Arc::from(catalog);
        let structure = self.indexes.remove(built).ok_or_else(|| api(format!("[EXTENSION] no index {built}")))?;
        if let Some(gone) = self.indexes.insert(name.to_owned(), structure) {
            gone.discard();
        }
        Ok(())
    }

    /// Index `name`'s options made `options`, as replayed: in place, or,
    /// if one fixed when it is made changes, with a structure made anew of
    /// its table's rows, never one opened, which has the old.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if there is no such index, table or method; the
    /// method's, making it; the table's, read.
    pub(super) fn replay_index_options(&mut self, name: &str, options: Vec<(String, Value)>) -> Result<()> {
        let ix = self.catalog.index(name).cloned().ok_or_else(|| api(format!("[EXTENSION] no index {name}")))?;
        if !needs_rebuild(&self.index_options_of(&ix.method), &ix.options, &options) {
            return self.set_index_options(name, options);
        }
        let table = self.catalog.table(&ix.table).cloned().ok_or_else(|| api(format!("[EXTENSION] no table {}", ix.table)))?;
        let method = Arc::clone(self.methods.get(&ix.method).ok_or_else(|| api(format!("[EXTENSION] no index method {}", ix.method)))?);
        let next = IndexDesc { options, ..ix };
        let structure: Arc<dyn Index> = Arc::from(method.create_on(&table.types(&next.columns).unwrap_or_default(), (&table, &next))?);
        // Opened from its files, it is filled once the tables are (`Database::reopen`).
        if !self.leave_unbuilt(name) {
            self.fill_index(&*structure, &table.name, &next.columns, Timestamp::MAX)?;
        }
        let mut catalog = self.catalog.duplicate();
        catalog.remove_index(name);
        catalog.add_index(next);
        self.catalog = Arc::from(catalog);
        if let Some(gone) = self.indexes.insert(name.to_owned(), structure) {
            gone.discard();
        }
        Ok(())
    }

    /// Index `name`'s options made `options`, its structure told.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if there is no such index, or table of it.
    pub(super) fn set_index_options(&mut self, name: &str, options: Vec<(String, Value)>) -> Result<()> {
        let ix = self.catalog.index(name).cloned().ok_or_else(|| api(format!("[EXTENSION] no index {name}")))?;
        let table = self.catalog.table(&ix.table).cloned().ok_or_else(|| api(format!("[EXTENSION] no table {}", ix.table)))?;
        let next = IndexDesc { options, ..ix };
        let mut catalog = self.catalog.duplicate();
        catalog.remove_index(name);
        catalog.add_index(next.clone());
        self.catalog = Arc::from(catalog);
        self.index_of(name)?.reconfigure((&table, &next));
        Ok(())
    }

    /// \[EXTENSION\] Drops the index `name`: out of the catalog and the
    /// state, its structure discarded (`Index::discard`), so its storage is
    /// released once no snapshot holds it.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if there is no such index, it enforces a UNIQUE
    /// constraint [X3.135-1986 §6.4 GR2], or it is being built.
    pub(super) fn drop_index(&mut self, name: &str) -> Result<()> {
        let desc = self.catalog.index(name).ok_or_else(|| api(format!("[EXTENSION] no index {name}")))?;
        if desc.unique {
            return fail(
                Code::Api,
                format!("[EXTENSION] the index {name} enforces a UNIQUE constraint of {} [X3.135-1986 §6.4 GR2]", desc.table),
            );
        }
        if self.building.contains(name) {
            return fail(Code::Api, format!("[EXTENSION] the index {name} is being built"));
        }
        let table = desc.table.clone();
        let mut catalog = self.catalog.duplicate();
        catalog.remove_index(name);
        self.catalog = Arc::from(catalog);
        if let Some(index) = self.indexes.remove(name) {
            index.discard();
        }
        // A statement whose snapshot still has the index must not write
        // the table (`State::prepare_write`).
        self.lock_of(&table)?.definitions_changed(self.generation);
        Ok(())
    }

    /// Takes the index `name` out of the catalog: its build failed. Its
    /// structure stays, for the undo of the writes that kept it up to date.
    fn forget_index(&mut self, name: &str) {
        let mut catalog = self.catalog.duplicate();
        catalog.remove_index(name);
        self.catalog = Arc::from(catalog);
        self.building.remove(name);
    }
}

impl Database {
    /// \[EXTENSION\] Create a secondary index named `name` on `columns` of
    /// `table` ("AUTH.TABLE"), with the index method `method`, while
    /// transactions go on reading and writing the table (see the module's
    /// documentation). A transaction that wrote the table before is refused
    /// (SQLCODE -911) at its next statement or its commit, to run again.
    ///
    /// # Errors
    ///
    /// SQLCODE -911 if another transaction holds the table's lock
    /// exclusively (for a table whose engine leaves concurrency to the
    /// coordinator, holds it at all) and locking is no-wait; SQLCODE
    /// -910 if the table, a column, the method or the name is wrong; the
    /// table's errors, reading its rows.
    pub fn create_index(&self, name: &str, table: &str, columns: &[&str], method: &str) -> Result<()> {
        let columns = columns.iter().map(|c| (*c).to_owned()).collect();
        let spec =
            IndexSpec { name: name.to_owned(), table: qualified_name(table)?, columns, method: method.to_owned(), options: Vec::new() };
        self.noted(self.make_index(&spec))
    }

    /// `create_index`'s work, and CREATE INDEX's (`db/index_statements.rs`).
    pub(super) fn make_index(&self, spec: &IndexSpec) -> Result<()> {
        self.build_index(spec, &|st, tx| st.log_definition(Some(tx), spec.change()))
    }

    /// Builds the index `spec` describes, as the module's documentation
    /// says, `ready` making the last change, with the gate open, once it is
    /// built: logging it, or putting it in another's place. If `ready`
    /// fails, the index is taken out.
    fn build_index(&self, spec: &IndexSpec, ready: &dyn Fn(&mut State, &Txn) -> Result<()>) -> Result<()> {
        let (name, qn) = (spec.name.as_str(), &spec.table);
        let current = self.snapshot()?;
        // [ISO/IEC 9075:1992 §4.9] Of a temporary table, only defined: each
        // session's instance makes its entries, in memory, of its own rows,
        // once no transaction has them open (`db/temporary/storage.rs`).
        if current.catalog.table(qn).is_some_and(|t| t.temporary.is_some()) {
            if spec.method != BTREE_METHOD && spec.method != HASH_METHOD {
                return Err(SqlError::new(
                    Code::SyntaxRule,
                    format!(
                        "[DESIGN] {qn} is temporary: its index is of method {BTREE_METHOD} or {HASH_METHOD}, kept in memory (RULES.md D49)"
                    ),
                ));
            }
            return self.create_index_locked(spec, ready);
        }
        if current.capabilities(qn)?.concurrency == Concurrency::Coordinator {
            return self.create_index_locked(spec, ready);
        }
        // Shared with the table's writers, but not with a transaction
        // dropping or changing the table (`db/ddl.rs`); taken before the gate
        // is shut, so a wait for it never holds the gate.
        let holder = current.begin();
        current.acquire(&holder, qn, LockMode::Shared)?;
        // 1. Published shut; the build's snapshot taken there.
        let shut = current.tx.gate.shut();
        let tx = current.begin();
        let defined = self.change(|st| {
            let (index, positions, _) = st.define_index(spec)?;
            st.building.insert(name.to_owned());
            Ok((index, positions, Building::begin(st, qn)?))
        });
        drop(shut);
        let (index, positions, building) = defined?;
        // 2. Filled.
        let filled = self.snapshot().and_then(|st| st.fill_index(&*index, qn, &positions, tx.snapshot()));
        // 3. Ready, or taken out.
        let done = match filled.and_then(|()| {
            self.change(|st| {
                st.building.remove(name);
                ready(st, &tx)
            })
        }) {
            Ok(()) => Ok(()),
            Err(e) => self
                .change(|st| {
                    st.forget_index(name);
                    Ok(())
                })
                .and(Err(e)),
        };
        drop(building);
        drop(holder);
        done
    }

    /// \[EXTENSION\] Drop the index named `name`. It waits for, or under
    /// `LockWait::NoWait` is refused by, the transactions writing its
    /// table, as it takes the table's exclusive lock: none of them can then
    /// have a change of the index to undo. Readers of earlier snapshots go
    /// on reading it; its storage is released when they are done.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if there is no such index, it enforces a UNIQUE
    /// constraint, or it is being built; SQLCODE -911 if another
    /// transaction holds its table's lock and locking is no-wait, or waiting
    /// would deadlock.
    pub fn drop_index(&self, name: &str) -> Result<()> {
        let current = self.snapshot()?;
        let table = current.catalog.index(name).map(|ix| ix.table.clone()).ok_or_else(|| api(format!("[EXTENSION] no index {name}")))?;
        let tx = current.begin();
        current.acquire(&tx, &table, LockMode::Exclusive)?;
        let _gate = current.tx.gate.enter();
        self.change(|st| {
            st.drop_index(name)?;
            st.log_definition(Some(&tx), Change::DropIndex { name: name.into() })
        })
    }

    /// Creates an index under the table's exclusive lock, held by a
    /// transaction of its own, so no open transaction has an uncommitted
    /// change to it: for an engine leaving concurrency to the coordinator,
    /// whose tables keep no versions.
    fn create_index_locked(&self, spec: &IndexSpec, ready: &dyn Fn(&mut State, &Txn) -> Result<()>) -> Result<()> {
        // The table's lock is shared by every snapshot, so this one's holds
        // it while the new snapshot is published; dropping the transaction
        // afterwards releases it.
        let current = self.snapshot()?;
        let tx = current.begin();
        current.acquire(&tx, &spec.table, LockMode::Exclusive)?;
        let _gate = current.tx.gate.enter();
        self.change(|st| {
            st.create_index(spec)?;
            ready(st, &tx)
        })
    }

    /// \[EXTENSION\] Sets the options of index `name` that `given` gives, a
    /// value or none for its default again (RULES.md D41), the change
    /// logged. If only options that change in place change, the index is
    /// told (`Index::reconfigure`) as the change is published. If one fixed
    /// when the index is made changes, the index is built again with them,
    /// while transactions go on writing its table, as CREATE INDEX builds
    /// one; readers go on reading the old until the new takes its place
    /// and name.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if there is no such index; -103 for an option not
    /// declared, or a value not of its kind; as `create_index`, building it.
    pub(super) fn alter_index(&self, name: &str, given: &[(String, Option<Value>)]) -> Result<()> {
        let current = self.snapshot()?;
        let ix = current.catalog.index(name).cloned().ok_or_else(|| api(format!("[EXTENSION] no index {name}")))?;
        let declared = current.index_options_of(&ix.method);
        let mut options = ix.options.clone();
        for (option, value) in given {
            options = chosen(&ix.method, (&declared, &options), (option, value.as_ref()))?;
        }
        let change = Change::AlterIndex { name: name.to_owned(), options: options.clone() };
        if needs_rebuild(&declared, &ix.options, &options) {
            let built = format!("{name} (rebuilt)");
            if current.catalog.index(&built).is_some() {
                return fail(Code::Api, format!("[EXTENSION] index {name} is built again as {built}, the name of another index"));
            }
            let table = current.catalog.table(&ix.table).ok_or_else(|| api(format!("[EXTENSION] no table {}", ix.table)))?;
            let columns = ix.columns.iter().filter_map(|&c| table.columns.get(c)).map(|c| c.name.clone()).collect();
            let spec = IndexSpec { name: built.clone(), table: ix.table.clone(), columns, method: ix.method.clone(), options };
            return self.build_index(&spec, &|st, tx| {
                st.swap_index((&built, name))?;
                st.log_definition(Some(tx), change.clone())
            });
        }
        let tx = current.begin();
        current.acquire(&tx, &ix.table, LockMode::Exclusive)?;
        let _gate = current.tx.gate.enter();
        self.change(|st| {
            st.set_index_options(name, options.clone())?;
            st.log_definition(Some(&tx), change.clone())
        })
    }
}

// Unit tests: an index built again that cannot take its place (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
