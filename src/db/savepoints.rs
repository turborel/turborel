//! [ISO/IEC 9075-2:2023 §4.43.2, §17.5, §17.6, §17.8] A session's
//! savepoints and savepoint levels (Features T271, T272).
//!
//! A session has a savepoint level even with no transaction active. A new
//! one is established, current until it is destroyed, as a function or a
//! procedure of NEW SAVEPOINT LEVEL is invoked [§9.18 GR7, GR15], as a
//! triggered action runs [§15.21 GR6c, GR6j], and as an ATOMIC compound
//! statement runs [ISO/IEC 9075-4:2006 draft §14.1 GR1, GR6]. A savepoint
//! exists at the level current as it was established, and a statement
//! names only those of the level current as it runs. Destroying a level
//! destroys its savepoints, what was changed since each staying.
//!
//! Each savepoint is one of the transaction's (`txn/savepoints.rs`),
//! with what the session itself undoes going back to it: the changes of
//! definitions staged after it (`db/ddl.rs`), and the locators given
//! after it [§17.8 GR3f]. Every open cursor is closed as one is gone back
//! to [GR3g], those opened before it too (IA168), and no prepared
//! statement is deallocated (IA167), nor are the diagnostics and
//! descriptor areas changed but as the statement completes (IA211). As
//! the transaction ends its savepoints go with it [§17.7 GR7, §17.8 GR2c].
//!
//! \[DESIGN\] Going back to a savepoint established before the statement
//! running began, from a routine's body, cancels no change of definitions,
//! as that statement runs on the definitions it began with: it is refused
//! if one was staged after the savepoint.

use super::{Call, Completion, Session, StatementContext};
use crate::ast::SavepointStatement;
use crate::error::{Code, Result, SqlError, SqlState};
use crate::sync::lock_builtin;
use crate::txn::{TxId, Txn};

/// [ISO/IEC 9075-2:2023 §17.5 GR3] The maximum number of savepoints per
/// SQL-transaction, implementation-defined (IL208).
pub(in crate::db) const MAX_SAVEPOINTS: usize = 1024;

/// What establishes a savepoint level beyond the session's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::db) enum Level {
    /// [ISO/IEC 9075-4:2006 draft §14.1 GR1] An ATOMIC compound statement.
    Atomic,
    /// [§9.18 GR7] An SQL-invoked function.
    Function,
    /// [§9.18 GR7] An SQL-invoked procedure of NEW SAVEPOINT LEVEL.
    Procedure,
    /// [§15.21 GR6c] A triggered action.
    Trigger,
}

impl Level {
    /// Whether it runs in an atomic execution context [§4.41.5]: all but a
    /// procedure, which a CALL, not atomic, invokes.
    fn atomic(self) -> bool {
        match self {
            Level::Atomic | Level::Function | Level::Trigger => true,
            Level::Procedure => false,
        }
    }
}

/// A savepoint, by its name, at its level, with what is `T` of it.
struct Entry<T> {
    name: String,
    /// Its level: how many were established beyond the session's own when
    /// it was.
    level: usize,
    /// How many atomic execution contexts were active as it was
    /// established [§17.8 GR3c].
    atomic: usize,
    held: T,
}

/// [ISO/IEC 9075-2:2023 §4.43.2] The savepoint levels established beyond
/// a session's own, innermost last, and the savepoints, oldest first,
/// each with what is `T` of it.
pub(in crate::db) struct Levels<T> {
    levels: Vec<Level>,
    named: Vec<Entry<T>>,
}

impl<T> Default for Levels<T> {
    fn default() -> Self {
        Levels { levels: Vec::new(), named: Vec::new() }
    }
}

impl<T> Levels<T> {
    /// A new savepoint level established, current.
    pub(in crate::db) fn open(&mut self, level: Level) {
        self.levels.push(level);
    }

    /// The current savepoint level destroyed, with its savepoints [§9.18
    /// GR15, §15.21 GR6j]: what is `T` of each, oldest first.
    pub(in crate::db) fn close(&mut self) -> Vec<T> {
        self.levels.pop();
        let depth = self.levels.len();
        let at = self.named.iter().position(|e| e.level > depth).unwrap_or(self.named.len());
        self.named.split_off(at).into_iter().map(|e| e.held).collect()
    }

    /// Whether a triggered action is executing: a level of one is
    /// established.
    pub(in crate::db) fn triggered(&self) -> bool {
        self.levels.contains(&Level::Trigger)
    }

    /// How many atomic execution contexts establish levels.
    fn atomic(&self) -> usize {
        self.levels.iter().filter(|l| l.atomic()).count()
    }

    /// Only the savepoints of which `keep` holds kept: what is `T` of
    /// those not.
    pub(in crate::db) fn retain(&mut self, keep: impl Fn(&T) -> bool) -> Vec<T> {
        let (kept, gone) = std::mem::take(&mut self.named).into_iter().partition(|e| keep(&e.held));
        self.named = kept;
        gone.into_iter().map(|e: Entry<T>| e.held).collect()
    }

    /// [ISO/IEC 9075-2:2023 §17.5 GR2, GR3] The savepoint `name` of the
    /// current level destroyed, if there is one, and room left for one
    /// more: what is `T` of the one destroyed.
    ///
    /// # Errors
    ///
    /// SQLSTATE 3B002 if `MAX_SAVEPOINTS` exist.
    pub(in crate::db) fn make_room(&mut self, name: &str) -> Result<Option<T>> {
        let replaced = self.find(name).map(|at| self.named.remove(at).held);
        if self.named.len() >= MAX_SAVEPOINTS {
            let msg = format!("[ISO/IEC 9075-2:2023 §17.5 GR3] the transaction has {MAX_SAVEPOINTS} savepoints, the most it may (IL208)");
            return Err(SqlError::new(Code::TransactionState, msg).with_state(SqlState::SAVEPOINT_TOO_MANY));
        }
        Ok(replaced)
    }

    /// [§17.5 GR4] The savepoint `name` established at the current level,
    /// `held` of it.
    pub(in crate::db) fn establish(&mut self, name: &str, held: T) {
        let (level, atomic) = (self.levels.len(), self.atomic());
        self.named.push(Entry { name: name.to_owned(), level, atomic, held });
    }

    /// The place of the savepoint `name` among them, if one of the
    /// current level is so named.
    fn find(&self, name: &str) -> Option<usize> {
        let level = self.levels.len();
        self.named.iter().position(|e| e.level == level && e.name == name)
    }

    /// [ISO/IEC 9075-2:2023 §17.6 GR2, GR3] The savepoint `name` of the
    /// current level destroyed, and every one established after it: what
    /// is `T` of each, oldest first.
    ///
    /// # Errors
    ///
    /// SQLSTATE 3B001 if the current level has none so named.
    pub(in crate::db) fn release(&mut self, name: &str) -> Result<Vec<T>> {
        let at = self.find(name).ok_or_else(|| invalid("§17.6 GR2", name))?;
        Ok(self.named.split_off(at).into_iter().map(|e| e.held).collect())
    }

    /// [ISO/IEC 9075-2:2023 §17.8 GR3b, GR3c] The savepoint `name` a
    /// ROLLBACK TO SAVEPOINT goes back to: what is `T` of it.
    ///
    /// # Errors
    ///
    /// SQLSTATE 3B001 if the current level has none so named, or it was
    /// established before the most recent atomic execution context began.
    pub(in crate::db) fn target(&self, name: &str) -> Result<&T> {
        let entry = self.find(name).and_then(|at| self.named.get(at)).ok_or_else(|| invalid("§17.8 GR3b", name))?;
        if entry.atomic < self.atomic() {
            return Err(invalid("§17.8 GR3c", name));
        }
        Ok(&entry.held)
    }

    /// [ISO/IEC 9075-2:2023 §17.8 GR3e] Every savepoint established after
    /// the savepoint `name` of the current level destroyed: what is `T`
    /// of each, oldest first.
    pub(in crate::db) fn after(&mut self, name: &str) -> Vec<T> {
        let at = self.find(name).map_or(self.named.len(), |at| at.saturating_add(1));
        self.named.split_off(at).into_iter().map(|e| e.held).collect()
    }
}

/// [ISO/IEC 9075-2:2023 §17.6 GR2, §17.8 GR3b, GR3c] SQLSTATE 3B001,
/// savepoint exception, invalid specification: `rule` finds no savepoint
/// `name` to name.
pub(in crate::db) fn invalid(rule: &str, name: &str) -> SqlError {
    let msg = format!("[ISO/IEC 9075-2:2023 {rule}] no savepoint {name} is established at the current savepoint level");
    SqlError::new(Code::TransactionState, msg).with_state(SqlState::SAVEPOINT_INVALID_SPECIFICATION)
}

/// SQLSTATE 3B001 of RELEASE or ROLLBACK TO `s` with no transaction
/// active, so no savepoint [§17.6 GR2, §17.8 GR3b].
pub(in crate::db) fn none_named(s: &SavepointStatement) -> SqlError {
    let rule = if let SavepointStatement::Release(_) = s { "§17.6 GR2" } else { "§17.8 GR3b" };
    invalid(rule, s.name())
}

/// What a session's savepoint is, beside its name: its transaction, the
/// transaction's savepoint, how many changes of definitions were staged,
/// and how many locators given, when it was established.
pub(in crate::db) struct Held {
    tx: TxId,
    id: u64,
    staged: usize,
    locators: u64,
}

impl Session {
    /// [ISO/IEC 9075-2:2023 §17.5, §17.6, §17.8] The savepoint's statement
    /// `s`, run directly in `call`: SAVEPOINT initiates a transaction if
    /// none is active [§4.41.2.4]; with none, no savepoint exists to name.
    ///
    /// # Errors
    ///
    /// As `savepoint_statement`'s; SQLSTATE 3B001 of RELEASE or ROLLBACK
    /// TO with no transaction active.
    pub(super) fn savepoint_direct(&self, call: &Call<'_>, s: &SavepointStatement) -> Result<Completion> {
        let st = call.state();
        let tx = match s {
            SavepointStatement::Establish(_) => self.ensure_transaction(st),
            SavepointStatement::Release(_) | SavepointStatement::RollbackTo(_) => {
                self.current_transaction().ok_or_else(|| none_named(s))?
            }
        };
        call.runs(&tx);
        self.savepoint_statement(StatementContext { st, tx: &tx }, s, false)
    }

    /// The savepoint's statement `s` in `cx`, `within` the statement
    /// running if it is in a routine's body.
    ///
    /// # Errors
    ///
    /// SQLSTATE 0W000 while a triggered action executes [§15.21 GR6e
    /// iii]; 3B001 if no savepoint `s` names is at the current level;
    /// 3B002 past `MAX_SAVEPOINTS`; SQLCODE -900 if a ROLLBACK TO within
    /// would cancel a change of definitions; -914 if a change could not
    /// be undone.
    pub(super) fn savepoint_statement(&self, cx: StatementContext<'_>, s: &SavepointStatement, within: bool) -> Result<Completion> {
        if self.nesting.savepoints.borrow().triggered() {
            let msg = "[ISO/IEC 9075-2:2023 §15.21 GR6e iii] a savepoint's statement while a triggered action executes";
            return Err(SqlError::new(Code::TransactionState, msg).with_state(SqlState::PROHIBITED_STATEMENT_IN_TRIGGER));
        }
        // Those of a transaction ended, or cancelled with a mark, are gone.
        let gone = self.nesting.savepoints.borrow_mut().retain(|h| h.tx == cx.tx.id && cx.tx.has_savepoint(h.id));
        drop(gone);
        match s {
            SavepointStatement::Establish(name) => self.establish(cx.tx, name),
            SavepointStatement::Release(name) => {
                let released = self.nesting.savepoints.borrow_mut().release(name)?;
                for h in &released {
                    cx.tx.forget_savepoint(h.id);
                }
                Ok(())
            }
            SavepointStatement::RollbackTo(name) => self.rollback_to(cx, name, within),
        }
        .map(|()| Completion::Success)
    }

    /// [ISO/IEC 9075-2:2023 §17.5 GR2 to GR4] SAVEPOINT `name` in `tx`.
    fn establish(&self, tx: &Txn, name: &str) -> Result<()> {
        if let Some(replaced) = self.nesting.savepoints.borrow_mut().make_room(name)? {
            tx.forget_savepoint(replaced.id);
        }
        // Taken with no borrow held, as it calls the engines (RULES.md
        // R-ERR-4).
        let id = tx.savepoint()?;
        let staged = self.inner.borrow().staged.as_ref().map_or(0, super::ddl::Staged::len);
        let locators = self.locators.borrow().issued();
        self.nesting.savepoints.borrow_mut().establish(name, Held { tx: tx.id, id, staged, locators });
        Ok(())
    }

    /// [ISO/IEC 9075-2:2023 §17.8 GR3] ROLLBACK TO SAVEPOINT `name` in
    /// `cx`, `within` the statement running if in a routine's body.
    fn rollback_to(&self, cx: StatementContext<'_>, name: &str, within: bool) -> Result<()> {
        let (id, staged, locators) = {
            let levels = self.nesting.savepoints.borrow();
            let held = levels.target(name)?;
            (held.id, held.staged, held.locators)
        };
        let now = self.inner.borrow().staged.as_ref().map_or(0, super::ddl::Staged::len);
        if within && now > staged {
            let msg = format!(
                "[DESIGN] ROLLBACK TO SAVEPOINT {name} in a routine's body would cancel changes of definitions the statement that invoked it runs on: run it directly"
            );
            return Err(SqlError::new(Code::Unsupported, msg));
        }
        // [GR3e] Those after it destroyed; it stays.
        let after = self.nesting.savepoints.borrow_mut().after(name);
        for h in &after {
            cx.tx.forget_savepoint(h.id);
        }
        // [GR3d] Its rows first, then the definitions they were written in.
        let undone = cx.st.undo_to_savepoint(cx.tx, id);
        let cancelled = if now > staged { self.cancel_definitions(cx.tx, staged) } else { Ok(()) };
        // [GR3f] The locators given since, invalid.
        self.locators.borrow_mut().given_after(locators);
        // [GR3g, GR3h] Every cursor closed (IA168).
        self.close_cursors();
        undone.and(cancelled)
    }

    /// [ISO/IEC 9075-2:2023 §17.8 GR3d] The changes of definitions of
    /// `tx` from the `kept`th on cancelled: the definitions it sees made
    /// again with those before, and what the cancelled ones made, which no
    /// other transaction ever saw, discarded.
    fn cancel_definitions(&self, tx: &Txn, kept: usize) -> Result<()> {
        let published = self.db.snapshot()?;
        let made = {
            let mut inner = self.inner.borrow_mut();
            let Some(staged) = inner.staged.as_mut() else { return Ok(()) };
            let made = staged.cancel_from(kept, &published);
            if staged.len() == 0 {
                inner.staged = None;
            }
            made?
        };
        let storages: Vec<_> = made.iter().flat_map(super::alter::Made::storages).collect();
        tx.forget_participants(&storages);
        made.iter().for_each(super::alter::Made::discard);
        Ok(())
    }

    /// [ISO/IEC 9075-2:2023 §17.8 GR3g] Every open cursor closed, as the
    /// transaction's end closes them.
    fn close_cursors(&self) {
        let mut inner = self.inner.borrow_mut();
        for c in inner.cursors.values_mut() {
            c.open = None;
        }
        for rows in inner.anonymous.drain(..) {
            if let Some(rows) = rows.upgrade() {
                *lock_builtin(&rows) = None;
            }
        }
    }

    /// A new savepoint level established, of `level`.
    pub(in crate::db) fn open_level(&self, level: Level) {
        self.nesting.savepoints.borrow_mut().open(level);
    }

    /// [ISO/IEC 9075-2:2023 §9.18 GR15, §15.21 GR6j; ISO/IEC 9075-4:2006
    /// draft §14.1 GR6] The current savepoint level destroyed, with its
    /// savepoints: what was changed since each stays.
    pub(in crate::db) fn close_level(&self) {
        let gone = self.nesting.savepoints.borrow_mut().close();
        let Some(tx) = self.current_transaction() else { return };
        for held in gone.iter().filter(|h| h.tx == tx.id) {
            tx.forget_savepoint(held.id);
        }
    }
}

// Unit tests: names at levels, replaced, released and gone back to, the
// limit, atomic contexts (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
