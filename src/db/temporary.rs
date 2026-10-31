//! \[EXTENSION\] Temporary tables [ISO/IEC 9075:1992 §4.9, §11.3]: each
//! session's own instance of each.
//!
//! A temporary table is defined in a schema, as a persistent one is, and
//! its definition is logged and kept by checkpoints; but its rows are each
//! session's, "effectively empty at the start of an SQL-session" and gone
//! at its end [§4.9]. So the definitions a session's statements see
//! (`Session::with_temporaries`) give each temporary table the session's
//! own instance: its rows, its indexes and its lock, in place of those of
//! the definition, which stay empty. An instance's rows are made when a
//! transaction of the session first reads or writes it, its entries of
//! each index when its session first sees the index (`Rows_::entries`),
//! both in its pages, with the older versions its transactions keep
//! (`db/temporary/undo.rs`): a buffer pool of the session's MEMORY.WORK_BYTES,
//! of the database's budget, over a scratch file among the database's
//! temporary files (`db/temporary/pages.rs`). No change to it is logged:
//! none is recovered [§4.9]. A table of ON COMMIT DELETE ROWS is emptied
//! as each transaction ends [§14.3 GR3]: committed, it is as the DELETE
//! GR3 runs leaves it; rolled back, as it was before the transaction,
//! empty too. One of ON COMMIT PRESERVE ROWS is collected then, by its
//! session (`Instance::collect`). ALTER TABLE of one changes each
//! session's instance (`db/temporary/uses.rs`, `db/temporary/reshape.rs`).

mod pages;
mod reshape;
mod storage;
mod undo;
mod uses;

pub(crate) use uses::Uses;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::{Session, State, rule};
use crate::catalog::{IndexDesc, QualifiedName, TableDesc, TemporaryKind};
use crate::error::Result;
use crate::index::Index;
use crate::settings::{MEMORY_WORK_BYTES, work_bytes};
use crate::storage::TableStorage;
use crate::sync::lock_builtin;
use crate::txn::{TableLock, Timestamp, TxId, remove_entries};
use pages::Place;
use storage::Rows_;

/// A session's instances of the temporary tables, each by its table and
/// the generation of its definition.
type Instances = Mutex<HashMap<(QualifiedName, u32), Instance>>;

/// A session's instances of the temporary tables (`Uses` notes them); and
/// its declared local temporary tables, each with its indexes
/// (`db/declared.rs`).
#[derive(Default)]
pub(super) struct Temporaries {
    made: Arc<Instances>,
    pub(super) declared: Mutex<Vec<(TableDesc, Vec<IndexDesc>)>>,
}

/// One instance: its rows, with its entries of each index, and its lock.
#[derive(Clone)]
struct Instance {
    rows: Arc<Rows_>,
    lock: Arc<TableLock>,
}

impl Session {
    /// [ISO/IEC 9075:1992 §4.9] `published`, with the session's declared
    /// local temporary tables (`db/declared.rs`), each temporary table in it
    /// this session's own instance: one made for it the first time it is
    /// seen, with its entries of each index of the table (`Rows_::entries`);
    /// forgotten once its definition is gone or of an earlier generation,
    /// unless a transaction is altering it, which may roll back. Pages an
    /// instance makes from then on take the session's MEMORY.WORK_BYTES as
    /// it is now (`Session::instance_place`).
    pub(super) fn with_temporaries(&self, published: Arc<State>) -> Arc<State> {
        let st = self.with_declared(published);
        let temporary: Vec<TableDesc> = st.catalog.tables().into_iter().filter(|t| t.temporary.is_some()).cloned().collect();
        if temporary.is_empty() {
            lock_builtin(&self.temporaries.made).clear();
            return st;
        }
        st.temporaries.enlist(&self.temporaries.made);
        let altering = st.temporaries.altering();
        let place = self.instance_place(&st);
        let instances: Vec<(TableDesc, Instance)> = {
            let mut made = lock_builtin(&self.temporaries.made);
            made.retain(|(name, generation), _| {
                temporary.iter().any(|t| t.name == *name && (*generation >= t.generation || altering.contains(name)))
            });
            let mut instances = Vec::new();
            for desc in temporary {
                let key = (desc.name.clone(), desc.generation);
                let instance = made.entry(key).or_insert_with(|| Instance::of(&desc, (&place, &st.temporaries))).clone();
                instances.push((desc, instance));
            }
            instances
        };
        let mut copy = State::clone(&st);
        for (desc, instance) in instances {
            instance.rows.resize_pages(place.bytes);
            let indexes: Vec<IndexDesc> = st.catalog.indexes(&desc.name).into_iter().cloned().collect();
            for (name, entries) in instance.rows.entries(&indexes) {
                let entries: Arc<dyn Index> = entries;
                copy.indexes.insert(name, entries);
            }
            let rows: Arc<dyn TableStorage> = Arc::<Rows_>::clone(&instance.rows);
            copy.storage.insert(desc.name.clone(), rows);
            copy.locks.insert(desc.name, instance.lock);
        }
        Arc::new(copy)
    }

    /// [ISO/IEC 9075:1992 §14.3 GR3] Its transaction, `tx` if it had one,
    /// ended: each instance of ON COMMIT DELETE ROWS emptied, each other
    /// collected (`Instance::collect`), and what it altered of `st`'s
    /// temporary tables used again.
    pub(super) fn temporaries_ended(&self, st: &State, tx: Option<TxId>) {
        if let Some(tx) = tx {
            st.temporaries.ended(tx);
        }
        let instances: Vec<Instance> = lock_builtin(&self.temporaries.made).values().cloned().collect();
        for instance in instances {
            if instance.rows.desc.temporary.is_some_and(|t| t.delete_rows) {
                instance.rows.emptied();
            } else {
                instance.collect();
            }
        }
    }

    /// \[DESIGN\] Where an instance it makes of `st`'s tables keeps its
    /// pages (RULES.md D49): a pool of its MEMORY.WORK_BYTES, its own
    /// setting if it has one, of `st`'s budget, over a file of `st`'s
    /// temporary files.
    pub(super) fn instance_place(&self, st: &State) -> Place {
        let bytes = work_bytes(st.settings.effective(MEMORY_WORK_BYTES, &self.own.borrow()).0);
        Place { budget: Arc::clone(&st.memory), work: Arc::clone(&st.work), bytes }
    }

    /// [ISO/IEC 9075:1992 §11.10] Its instance of temporary table `name`,
    /// of generation `generation`, given entries, empty, of `indexes`: its
    /// transaction's ALTER TABLE added them, while no instance holds a row.
    pub(super) fn add_empty_entries(&self, (name, generation): (&QualifiedName, u32), indexes: &[IndexDesc]) {
        let found = lock_builtin(&self.temporaries.made).get(&(name.clone(), generation)).cloned();
        if let Some(instance) = found {
            instance.rows.add_empty(indexes);
        }
    }
}

/// [ISO/IEC 9075:1992 §11.8 SR4, SR5; §11.9 SR3, SR4] That `from` may
/// refer to `to`, as a foreign key or a CHECK does, the Syntax Rules
/// `kinds` and `rows` cited: a persistent table to persistent ones alone;
/// a global temporary table to global ones; a created local one to those,
/// or created local ones; a declared one to any temporary one; and one
/// whose rows a commit keeps to none whose rows it deletes.
///
/// # Errors
///
/// SQLCODE -103 if it may not.
pub(super) fn may_refer(from: &TableDesc, to: &TableDesc, (kinds, rows): (&str, &str)) -> Result<()> {
    let (of, at) = (from.temporary.map(|t| t.kind), to.temporary.map(|t| t.kind));
    let kind_fits = match of {
        None => at.is_none(),
        Some(TemporaryKind::Global) => at == Some(TemporaryKind::Global),
        Some(TemporaryKind::CreatedLocal) => matches!(at, Some(TemporaryKind::Global | TemporaryKind::CreatedLocal)),
        Some(TemporaryKind::Declared) => at.is_some(),
    };
    if !kind_fits {
        return Err(rule(format!(
            "[ISO/IEC 9075:1992 {kinds}] {} may not refer to {}: a {} table, to a {} one",
            from.name,
            to.name,
            kind_of(of),
            kind_of(at)
        )));
    }
    let deletes = |t: &TableDesc| t.temporary.is_some_and(|t| t.delete_rows);
    if deletes(to) && !deletes(from) {
        return Err(rule(format!(
            "[ISO/IEC 9075:1992 {rows}] {} keeps its rows at commit, so may not refer to {}, whose rows a commit deletes",
            from.name, to.name
        )));
    }
    Ok(())
}

/// How a table of `kind` (none: persistent) is called.
fn kind_of(kind: Option<TemporaryKind>) -> &'static str {
    match kind {
        None => "persistent base",
        Some(TemporaryKind::Global) => "global temporary",
        Some(TemporaryKind::CreatedLocal) => "created local temporary",
        Some(TemporaryKind::Declared) => "declared local temporary",
    }
}

impl Instance {
    /// \[DESIGN\] Removes every version of its rows superseded or deleted,
    /// their index entries with them, as its session's transaction has
    /// ended: no transaction of another session reads an instance, and none
    /// of its own is open, so none can read them, whatever the database's
    /// horizon (RULES.md D49). A failure leaves them for its next end.
    fn collect(&self) {
        let Some(rows) = self.rows.made() else { return };
        let entries = self.rows.all_entries();
        let indexes: Vec<(&dyn Index, &[usize])> = entries
            .iter()
            .map(|entries| {
                let index: &dyn Index = &**entries;
                (index, entries.index.columns.as_slice())
            })
            .collect();
        let _ = rows.gc(Timestamp::MAX, &mut |id, row| remove_entries(&indexes, id, &row));
    }

    /// An instance of `desc`, its rows and entries made as they are first
    /// used, in pages at `place`, written as `uses` lets.
    fn of(desc: &TableDesc, (place, uses): (&Place, &Arc<Uses>)) -> Instance {
        Instance { rows: Arc::new(Rows_::new(desc, place, uses)), lock: Arc::default() }
    }

    /// `st`, with this instance as table `name`'s: its rows and lock, and,
    /// if `written`, its entries of each index `st` gives the table.
    fn on(&self, st: &State, name: &QualifiedName, written: bool) -> Arc<State> {
        let mut copy = State::clone(st);
        if written {
            let indexes: Vec<IndexDesc> = st.catalog.indexes(name).into_iter().cloned().collect();
            for (index, entries) in self.rows.entries(&indexes) {
                let entries: Arc<dyn Index> = entries;
                copy.indexes.insert(index, entries);
            }
        }
        let rows: Arc<dyn TableStorage> = Arc::<Rows_>::clone(&self.rows);
        copy.storage.insert(name.clone(), rows);
        copy.locks.insert(name.clone(), Arc::clone(&self.lock));
        Arc::new(copy)
    }
}

// Unit tests: an instance's collection (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
