//! [ISO/IEC 9075-2:2023 §15.21 GR6, §9.18 GR6] The context a triggered
//! action, or an SQL-invoked procedure invoked, pushes on its session.
//!
//! As its statements run, the session's diagnostics area is one of its
//! own, discarded after, so the statement that caused it reads its own
//! [§15.21 GR6a, GR6k; §9.18 GR6f, GR14]. The top cell of its
//! authorization stack holds the owner of its schema alone, a user
//! identifier or a role name [§15.21 GR6b; §9.18 GR6i i, GR6l ii 1,
//! SR23b; §4.42.1.2], the SQL-session user identifier staying as it was;
//! its
//! default schema the one its names were defined in, and no session
//! collation applies, as of a view. What the session was is put back as
//! the context is popped.
//!
//! A triggered action and a function establish a new savepoint level as
//! they run, and a procedure of NEW SAVEPOINT LEVEL too, destroyed as the
//! context is popped [§15.21 GR6c, GR6j; §9.18 GR7, GR15]
//! (`db/savepoints.rs`).
//!
//! \[DESIGN\] Triggered actions and procedures nest, each a statement of
//! the one it is nested in, at most `MAX_NESTING` deep (RULES.md D71).

use std::cell::{Cell, RefCell};

use super::collations::Collating;
use super::savepoints::{Held, Level, Levels};
use super::{Session, area};
use crate::catalog::QualifiedName;
use crate::debugger::{Kind, Origin};
use crate::error::{Code, Result, SqlError};

/// \[DESIGN\] How deep triggered actions and SQL-invoked procedures nest,
/// a statement of one activating a trigger or invoking a procedure: past
/// it, the statement fails, as with any exception of the action or the
/// procedure, not the stack of the thread that runs it (RULES.md R-CF-6).
pub(in crate::db) const MAX_NESTING: usize = 16;

/// The triggers and procedures whose contexts a session has pushed,
/// innermost last.
#[derive(Default)]
pub(in crate::db) struct Nesting {
    executing: RefCell<Vec<QualifiedName>>,
    /// [ISO/IEC 9075-4:2006 draft §14.1 SR15] The programs the session has
    /// run, which numbers each execution apart (`params::local_key`).
    executions: Cell<usize>,
    /// Its savepoint levels and savepoints (`db/savepoints.rs`).
    pub(in crate::db) savepoints: RefCell<Levels<Held>>,
}

impl Nesting {
    /// How many contexts are pushed.
    pub(in crate::db) fn depth(&self) -> usize {
        self.executing.borrow().len()
    }

    /// A number no program the session has run has as its execution.
    pub(in crate::db) fn execution(&self) -> usize {
        let n = self.executions.get().wrapping_add(1);
        self.executions.set(n);
        n
    }
}

/// What a session was, before a context was pushed on it: put back as it
/// is dropped.
pub(in crate::db) struct Pushed<'s> {
    session: &'s Session,
    auth: String,
    role: Option<String>,
    schema: String,
    collating: Collating,
    area: area::Area,
    /// Whether the context established a savepoint level.
    level: bool,
}

impl Drop for Pushed<'_> {
    fn drop(&mut self) {
        let s = self.session;
        *s.auth.borrow_mut() = std::mem::take(&mut self.auth);
        *s.role.borrow_mut() = self.role.take();
        *s.schema.borrow_mut() = std::mem::take(&mut self.schema);
        s.prepared_collation.replace(std::mem::replace(&mut self.collating, Collating::AsSet));
        s.inner.borrow_mut().area = std::mem::take(&mut self.area);
        s.nesting.executing.borrow_mut().pop();
        if self.level {
            s.close_level();
        }
    }
}

impl Session {
    /// \[DESIGN\] The trigger whose triggered action the session executes
    /// now, as a debugger names it: the context pushed last, of a program
    /// bound over transition variables (RULES.md D1004).
    pub(in crate::db) fn trigger_executing(&self) -> Option<Origin> {
        let name = self.nesting.executing.borrow().last().cloned()?;
        Some(Origin { kind: Kind::Trigger, shown: name.clone(), name })
    }

    /// Pushes the context of the trigger or procedure `name`, of the
    /// authorization identifier `owner` and the default schema `schema`,
    /// on the session, as the module says, a new savepoint level of
    /// `level` established if given: popped as what is returned is
    /// dropped.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 past `MAX_NESTING` contexts.
    pub(in crate::db) fn push(&self, name: &QualifiedName, (owner, schema): (String, String), level: Option<Level>) -> Result<Pushed<'_>> {
        if self.nesting.depth() >= MAX_NESTING {
            let msg =
                format!("[DESIGN] triggered actions and SQL-invoked procedures nest at most {MAX_NESTING} deep: {name} would nest deeper");
            return Err(SqlError::new(Code::Unsupported, msg));
        }
        self.nesting.executing.borrow_mut().push(name.clone());
        if let Some(level) = level {
            self.open_level(level);
        }
        Ok(Pushed {
            session: self,
            auth: self.auth.replace(owner),
            role: self.role.take(),
            schema: self.schema.replace(schema),
            collating: self.prepared_collation.replace(Collating::Fixed(None)),
            area: std::mem::take(&mut self.inner.borrow_mut().area),
            level: level.is_some(),
        })
    }
}
