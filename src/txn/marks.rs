//! [ISO/IEC 9075-2:2023 §4.41.5; ISO/IEC 9075-4:2006 draft §14.2 GR3b iii
//! 2] Marks within a statement: where a statement of a routine's body or a
//! triggered action begins, or an atomic compound statement, so that it
//! alone can be undone while the statement that executes it goes on.
//!
//! A statement's undo entries begin at its start (`Txn::begin_statement`).
//! A mark begins a segment of them within it, takes a savepoint of each
//! engine that undoes its own changes and has joined the statement, and
//! notes the place in the log of its first change, so that undoing to it
//! logs that change and those after it cancelled, as a failed statement's
//! are (`Change::StatementUndone`). An engine that joins after the mark
//! has a savepoint taken as it joins, which undoing goes back to.
//!
//! Marks nest: each is released or undone before the one it is within,
//! as the statements they mark run one inside another.

use std::mem;

use super::Txn;
use super::undo::Joined;
use crate::db::State;
use crate::error::{Result, SqlError};
use crate::log::record::{Change, Record};
use crate::memory::Undoing;
use crate::sync::lock_builtin;

/// A place within a statement to undo to.
pub(crate) struct Mark {
    /// Where its undo entries begin.
    undo: usize,
    /// A savepoint of each engine that had joined the statement when it was
    /// taken.
    savepoints: Vec<(Joined, u64)>,
    /// How many engines had joined the statement then.
    joined: usize,
    /// Its place among the marks open.
    depth: usize,
    /// The id the first savepoint taken within it has or would have.
    savepoints_from: u64,
}

impl Txn {
    /// A mark here, within the current statement.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if its lock is poisoned.
    pub(crate) fn mark(&self) -> Result<Mark> {
        let mut st = self.state()?;
        let undo = st.undo.mark();
        let joined = st.statement_savepoints.len();
        // Only the transaction's own session touches its bookkeeping, so it
        // is the same before and after each engine's `savepoint`.
        let savepoints = st.statement_savepoints.iter().map(|(j, _)| (j.clone(), j.storage.savepoint(self.id))).collect();
        st.marks_from.push(None);
        let (depth, within) = (st.marks_from.len().saturating_sub(1), st.savepoints.next());
        Ok(Mark { undo, savepoints, joined, depth, savepoints_from: within })
    }

    /// Releases `mark`: what it marked completed, and stays as the
    /// statement's.
    pub(crate) fn release_mark(&self, mark: Mark) {
        let depth = mark.depth;
        // Spent: a mark released is undone to no more.
        drop(mark);
        lock_builtin(&self.state).marks_from.truncate(depth);
    }

    /// Marks the changes since `from`, the place in the log of the first
    /// change after a mark, undone in the log, as `statement_undone` does a
    /// statement's.
    pub(super) fn mark_undone(&self, from: Option<crate::log::Lsn>) {
        let (Some(log), Some(from)) = (&self.log, from) else { return };
        let record = Record { tx: self.id, change: Change::StatementUndone { from } }.encode();
        // Appended with the state unlocked: the log is a plug-in (RULES.md
        // R-ERR-4).
        if log.append(&record).is_err() {
            lock_builtin(&self.state).doomed.get_or_insert("changes undone to a mark could not be logged as undone");
        }
    }
}

impl State {
    /// [ISO/IEC 9075-2:2023 §4.41.5] Cancels every change `tx` made since
    /// `mark`, failed with `failure`: what it marked is undone, and the
    /// statement it is within goes on.
    ///
    /// # Errors
    ///
    /// SQLCODE -914 if a change could not be undone; the error names
    /// `failure`.
    pub(crate) fn undo_to(&self, tx: &Txn, mark: Mark, failure: &SqlError) -> Result<()> {
        // Never refused by the memory budget (RULES.md D38).
        let _undoing = Undoing::begin();
        let _gate = self.tx.gate.enter();
        let (entries, joined_after, from) = {
            let mut st = tx.state()?;
            let entries = st.undo.take_from(mark.undo)?;
            let kept = mark.joined.min(st.statement_savepoints.len());
            let after = st.statement_savepoints.split_off(kept);
            let from = st.marks_from.get(mark.depth).copied().flatten();
            st.marks_from.truncate(mark.depth);
            // A savepoint taken within it is cancelled with it.
            st.savepoints.cancel_from(mark.savepoints_from);
            (entries, after, from)
        };
        let mut failures = entries.apply(tx.id);
        for (joined, sp) in mark.savepoints.into_iter().chain(joined_after) {
            if let Err(error) = joined.storage.rollback_to(tx.id, sp) {
                failures.push(super::UndoFailure { table: Some(crate::catalog::QualifiedName::clone(&joined.table)), error, rows: true });
            }
        }
        if !failures.is_empty() {
            tx.state()?.doomed = Some("a statement of the transaction could not be undone");
        }
        tx.mark_undone(from);
        self.report_damage(&failures, &format!("a statement that failed with \"{failure}\""))
    }
}

impl super::TxState {
    /// Notes `lsn`, the place in the log of a change, as the first after
    /// each open mark that has none yet.
    pub(super) fn logged_at(&mut self, lsn: crate::log::Lsn) {
        for from in self.marks_from.iter_mut().filter(|f| f.is_none()) {
            *from = Some(lsn);
        }
        self.savepoints.logged_at(lsn);
    }

    /// Forgets every mark, as a statement begins, which savepoints taken
    /// from now on are taken in (`txn/savepoints.rs`).
    pub(super) fn clear_marks(&mut self) {
        drop(mem::take(&mut self.marks_from));
        self.savepoints.statement_began();
    }
}
