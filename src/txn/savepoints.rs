//! [ISO/IEC 9075-2:2023 §4.43.2, §17.5 GR4, §17.8 GR3d] A transaction's
//! savepoints: places in it that ROLLBACK TO SAVEPOINT goes back to,
//! cancelling every change made since while the transaction goes on.
//!
//! A savepoint is taken as marks are (`txn/marks.rs`), but across
//! statements. It notes where the undo entries made after it begin, takes
//! a savepoint of each participant that undoes its own changes, and notes
//! the place in the log of the first change after it, so that going back
//! logs that change and those after it cancelled, as a failed statement's
//! are (`Change::StatementUndone`). An engine that undoes its own changes
//! and joins after it has a savepoint taken as it joins, which going back
//! returns to.
//!
//! Going back may cross the start of the statement running, as a
//! procedure's body goes back to a savepoint established before it was
//! called, or a savepoint taken earlier in it. So the statement's own
//! bookkeeping is clamped to the savepoint, and the savepoints its
//! statement took of engines after it are taken again. Its names, its
//! level and what else a session undoes are the session's
//! (`db/savepoints.rs`).

use super::undo::Joined;
use super::{LogPos, Txn, Undo, UndoFailure};
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::error::{Code, Result, SqlError};
use crate::memory::Undoing;
use crate::storage::TableStorage;
use crate::sync::lock_builtin;

/// One savepoint of a transaction.
struct Savepoint {
    /// Its id, given in order, never again in the transaction.
    id: u64,
    /// Where the undo entries made after it begin.
    undo: usize,
    /// A savepoint of each engine that undoes its own changes, of those
    /// joined when it was taken and those joined since.
    engines: Vec<(Joined, u64)>,
    /// The place of the first change after it, once one is made.
    from: Option<LogPos>,
    /// The statement it was taken in, by its number among those begun.
    statement: u64,
    /// How many savepoints that statement had taken of engines then.
    kept: usize,
}

/// \[DESIGN\] The savepoints a transaction has, oldest first.
#[derive(Default)]
pub(super) struct Savepoints {
    open: Vec<Savepoint>,
    /// The id the next savepoint is given.
    next: u64,
    /// How many statements have begun in the transaction.
    statement: u64,
}

impl Savepoints {
    /// A statement begins.
    pub(super) fn statement_began(&mut self) {
        self.statement = self.statement.wrapping_add(1);
    }

    /// Notes `at`, the place of a change, as the first after each
    /// savepoint that has none yet, the last ones: whether one took it.
    pub(super) fn logged_at(&mut self, at: LogPos) -> bool {
        let mut took = false;
        for savepoint in self.open.iter_mut().rev().take_while(|s| s.from.is_none()) {
            savepoint.from = Some(at);
            took = true;
        }
        took
    }

    /// The places of the first changes after the savepoints.
    pub(super) fn places(&mut self) -> impl Iterator<Item = &mut Option<LogPos>> {
        self.open.iter_mut().map(|s| &mut s.from)
    }

    /// The id the next savepoint will be given: those taken from now on
    /// have it or a later one.
    pub(super) fn next(&self) -> u64 {
        self.next
    }

    /// Forgets the savepoints whose id is `from` or later: taken within a
    /// mark undone, whose changes are cancelled with it.
    pub(super) fn cancel_from(&mut self, from: u64) {
        self.open.retain(|s| s.id < from);
    }
}

impl Txn {
    /// [ISO/IEC 9075-2:2023 §17.5 GR4] A savepoint here: its id.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if its lock is poisoned.
    pub(crate) fn savepoint(&self) -> Result<u64> {
        let joined = self.state()?.joined.clone();
        // Taken with the state unlocked, as engines are plug-ins (RULES.md
        // R-ERR-4); only the transaction's own session touches it.
        let engines = joined
            .into_iter()
            .filter(|j| j.storage.capabilities().undo == Undo::Native)
            .map(|j| {
                let sp = j.storage.savepoint(self.id);
                (j, sp)
            })
            .collect();
        let mut st = self.state()?;
        let (id, undo, kept) = (st.savepoints.next, st.undo.mark(), st.statement_savepoints.len());
        let statement = st.savepoints.statement;
        st.savepoints.next = id.wrapping_add(1);
        st.savepoints.open.push(Savepoint { id, undo, engines, from: None, statement, kept });
        Ok(id)
    }

    /// Whether savepoint `id` is one it has.
    pub(crate) fn has_savepoint(&self, id: u64) -> bool {
        lock_builtin(&self.state).savepoints.open.iter().any(|s| s.id == id)
    }

    /// [ISO/IEC 9075-2:2023 §17.5 GR2, §17.6 GR3] Savepoint `id`
    /// destroyed: what was changed since it stays.
    pub(crate) fn forget_savepoint(&self, id: u64) {
        lock_builtin(&self.state).savepoints.open.retain(|s| s.id != id);
    }

    /// Takes a savepoint of `joined`, an engine that undoes its own
    /// changes and has just joined, for each savepoint it has: going back
    /// to one returns the engine to where it began.
    pub(super) fn joined_after_savepoints(&self, joined: &Joined) {
        if lock_builtin(&self.state).savepoints.open.is_empty() {
            return;
        }
        let sp = joined.storage.savepoint(self.id);
        for savepoint in &mut lock_builtin(&self.state).savepoints.open {
            savepoint.engines.push((joined.clone(), sp));
        }
    }

    /// The participants whose storage is one of `storages`, made by a
    /// change of definitions a savepoint's going back cancelled, rolled
    /// back in their engines and no longer participants: the storage is
    /// discarded, never published.
    pub(crate) fn forget_participants(&self, storages: &[std::sync::Arc<dyn TableStorage>]) {
        let gone: Vec<Joined> = {
            let mut st = lock_builtin(&self.state);
            let (gone, kept) = std::mem::take(&mut st.joined).into_iter().partition(|j| storages.iter().any(|s| j.is(s)));
            st.joined = kept;
            st.statement_savepoints.retain(|(j, _)| !storages.iter().any(|s| j.is(s)));
            for savepoint in &mut st.savepoints.open {
                savepoint.engines.retain(|(j, _)| !storages.iter().any(|s| j.is(s)));
            }
            gone
        };
        for joined in gone {
            // Its rows were undone already, and nothing reads the storage
            // again: a failure here leaves nothing to damage.
            let _ = joined.storage.rollback(self.id);
        }
    }
}

/// What going back to a savepoint takes from the transaction's
/// bookkeeping: the undo entries after it, its engines' savepoints, where
/// its first change was logged, and the engines the statement running
/// took savepoints of after it.
type Taken = (super::UndoLog, Vec<(Joined, u64)>, Option<LogPos>, Vec<Joined>);

impl State {
    /// [ISO/IEC 9075-2:2023 §17.8 GR3d, GR3e] Cancels every change `tx`
    /// made since savepoint `id`, which stays, and forgets each savepoint
    /// taken after it. The transaction goes on.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `tx` has no savepoint `id`; -914 if a change could
    /// not be undone.
    pub(crate) fn undo_to_savepoint(&self, tx: &Txn, id: u64) -> Result<()> {
        // Never refused by the memory budget (RULES.md D38).
        let _undoing = Undoing::begin();
        let _gate = self.tx.gate.enter();
        let (entries, engines, from, stale) = taken(tx, id)?;
        let mut failures = entries.apply(tx.id);
        for (joined, sp) in engines {
            if let Err(error) = joined.storage.rollback_to(tx.id, sp) {
                failures.push(UndoFailure { table: Some(QualifiedName::clone(&joined.table)), error, rows: true });
            }
        }
        // Each engine the statement took a savepoint of after it is where
        // it was then, or where it began: its savepoint for the statement
        // is taken again there.
        let fresh: Vec<(Joined, u64)> = stale
            .into_iter()
            .map(|j| {
                let sp = j.storage.savepoint(tx.id);
                (j, sp)
            })
            .collect();
        tx.state()?.statement_savepoints.extend(fresh);
        if !failures.is_empty() {
            tx.state()?.doomed = Some("changes rolled back to a savepoint could not be undone");
        }
        tx.mark_undone(from);
        self.report_damage(&failures, "a rollback to a savepoint")
    }
}

/// What going back to savepoint `id` of `tx` takes from its bookkeeping
/// (`Taken`): the savepoints after it forgotten, its first change's place
/// in the log forgotten, the statement running clamped to it.
fn taken(tx: &Txn, id: u64) -> Result<Taken> {
    let mut st = tx.state()?;
    let Some(at) = st.savepoints.open.iter().position(|s| s.id == id) else {
        return Err(SqlError::new(Code::Api, format!("[DESIGN] the transaction has no savepoint {id}")));
    };
    st.savepoints.open.truncate(at.saturating_add(1));
    let current = st.savepoints.statement;
    let Some(savepoint) = st.savepoints.open.last_mut() else {
        return Err(SqlError::new(Code::Api, format!("[DESIGN] the transaction has no savepoint {id}")));
    };
    let (undo, engines, from) = (savepoint.undo, savepoint.engines.clone(), savepoint.from.take());
    let kept = if savepoint.statement == current { savepoint.kept } else { 0 };
    let entries = st.undo.take_from(undo)?;
    st.statement_start = st.statement_start.min(undo);
    let kept = kept.min(st.statement_savepoints.len());
    let stale = st.statement_savepoints.split_off(kept).into_iter().map(|(j, _)| j).collect();
    Ok((entries, engines, from, stale))
}

// Unit tests: undo to a savepoint across statements, engines joined after
// one, the statement's bookkeeping clamped (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
