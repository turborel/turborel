//! \[EXTENSION\] Indexes built whole by a load (RULES.md D532). As a load
//! begins, the indexes of its table that are empty, but unique ones, have
//! their entries held back (`txn/undo/loading.rs`): the table's lock is
//! taken alone first, so no other transaction writes it until this one
//! ends, and its rows are not seen by others' readers, so none of them
//! misses an entry. A load is one INSERT of a batch of rows or more, or a
//! bulk copy's executions, its last ending it. As it ends, each index held
//! back is built whole from its entries, sorted (`Index::load`), if it is
//! empty still; else, if a row went in by another way meanwhile, its
//! entries are inserted in their rows' order.
//!
//! A unique index is never held back, so a UNIQUE constraint is checked as
//! each row goes in, as before; and a table whose rows a trigger, a
//! foreign key or an assertion reads loads none, as `rows_checked` finds
//! (`integrity/inserted.rs`), so nothing reads an index held back before
//! the load ends.

use std::sync::Arc;

use super::{Indexed, Together};
use crate::db::State;
use crate::error::Result;
use crate::params::Load;
use crate::txn::{LockMode, Txn};

impl Indexed {
    /// Whether a load may hold back its entries: no unique index, and empty.
    fn loads(&self) -> bool {
        !self.desc.unique && self.index.holds_none()
    }
}

/// Holds back, for the load the rows of `together` are part of, the
/// entries of its indexes the load holds back: those of the load under way
/// into its table; else, as it begins, each that `loads`, the table's lock
/// taken alone first. None if a load into another table is under way.
///
/// # Errors
///
/// As `State::acquire`; SQLCODE -910 if the transaction's state is
/// poisoned.
pub(super) fn hold_back(st: &State, tx: &Txn, together: &mut Together<'_>) -> Result<()> {
    if let Some(held) = tx.held_back(together.table)? {
        together.grouped = grouped(tx, together)?;
        holding(together, &held);
        return Ok(());
    }
    let grouping = grouped(tx, together)?;
    if grouping.is_none() && !together.indexes.iter().any(Indexed::loads) {
        return Ok(());
    }
    let begun = st.acquire(tx, together.table, LockMode::Exclusive).and_then(|()| {
        // Under the lock, again: no other transaction's row went in meanwhile.
        together.grouped = grouping.map(|_| grouped(tx, together)).transpose()?.flatten();
        let held: Vec<_> = together.indexes.iter().filter(|ix| ix.loads()).map(|ix| Arc::clone(&ix.index)).collect();
        let begun = (!held.is_empty() || together.grouped.is_some()) && tx.begin_load(together.table, held.clone(), together.grouped)?;
        Ok(begun.then_some(held))
    });
    match begun {
        Ok(Some(held)) => holding(together, &held),
        Ok(None) => ungrouped(tx, together),
        Err(e) => {
            ungrouped(tx, together);
            return Err(e);
        }
    }
    Ok(())
}

/// Of the rows of `together`, how many a group takes if its storage writes
/// the load's rows straight as groups (RULES.md D533), as it begins or
/// goes on.
fn grouped(tx: &Txn, together: &Together<'_>) -> Result<Option<usize>> {
    let Some(loading) = together.storage.loading() else { return Ok(None) };
    let grouped = loading.begin_load(tx.id)?;
    if grouped.is_none() {
        loading.end_load(tx.id);
    }
    Ok(grouped)
}

/// The rows of `together` going to its tree, not as groups.
fn ungrouped(tx: &Txn, together: &mut Together<'_>) {
    if let Some(loading) = together.storage.loading() {
        loading.end_load(tx.id);
    }
    together.grouped = None;
}

/// Marks the indexes of `together` that `held` holds back.
fn holding(together: &mut Together<'_>, held: &[Arc<dyn crate::index::Index>]) {
    for ix in &mut together.indexes {
        ix.held = held.iter().any(|h| std::ptr::addr_eq(Arc::as_ptr(h), Arc::as_ptr(&ix.index)));
    }
}

impl State {
    /// Ends the load under way in `tx`, if one is: before a statement that
    /// is no part of it, or the transaction's commit.
    ///
    /// # Errors
    ///
    /// As `end`.
    pub(crate) fn end_load(&self, tx: &Txn) -> Result<()> {
        if !tx.loads()? {
            return Ok(());
        }
        end(self, tx, Some(Load::Ending))
    }
}

/// Ends the load under way, if `load` ends it: each index it held back
/// built whole from its entries if it is empty still, else given them in
/// their rows' order; checkpoints waiting meanwhile, so none writes an
/// index half built.
///
/// # Errors
///
/// As `Index::load` and `Index::insert_from`; as reading the entries back
/// (`Txn::end_load`).
pub(in crate::db) fn end(st: &State, tx: &Txn, load: Option<Load>) -> Result<()> {
    if !matches!(load, Some(Load::Alone | Load::Ending)) {
        return Ok(());
    }
    let _gate = st.tx.gate.enter();
    if let Some(table) = tx.loading_table()?
        && let Some(loading) = st.storage_of(&table)?.loading()
    {
        loading.end_load(tx.id);
    }
    tx.end_load(&mut |index, entries| {
        if index.holds_none() {
            return index.load(entries, (&st.work, st.load_area()));
        }
        for entry in entries {
            let (key, id) = entry?;
            index.insert_from(&key, id, false)?;
        }
        Ok(())
    })
}
