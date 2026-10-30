//! \[DESIGN\] A database's handles (RULES.md D55): the program's, its
//! sessions' among them, counted apart from those its own jobs hold while
//! they work, and those that do not keep it open. As the program's last
//! handle is dropped, every job's thread is stopped and joined, there, so
//! no job's thread drops the database last: it goes as the program's drop
//! returns, its files closed and its directory's lock given up then. Split
//! from database.rs (RULES.md R-SIZE-2).

use std::sync::{Arc, Weak};

use super::{Database, Shared};
use crate::diagnostics::{Diagnostics, Event};

/// What the program's handles share: the database, kept open while one is
/// held, and its jobs stopped as the last goes.
pub(in crate::db) struct Program {
    shared: Arc<Shared>,
}

impl Program {
    /// The program's first handle to the database `make` makes, given what
    /// its handles will share.
    pub(super) fn over(make: impl FnOnce(Weak<Program>) -> Shared) -> Database {
        let program = Arc::new_cyclic(|me| Program { shared: Arc::new(make(Weak::clone(me))) });
        Database(Arc::clone(&program.shared), Some(program))
    }
}

/// The program's last handle dropped: every job's thread is stopped and
/// joined here, a job's work in progress waited for, before the database
/// the program held goes. A job's handle is no program's, so this never
/// runs on a job's thread, which would have none to join it.
impl Drop for Program {
    fn drop(&mut self) {
        Database(Arc::clone(&self.shared), None).background_closing();
    }
}

/// A handle to a database that does not keep it open: what a thread of the
/// database's own holds (`threads.rs`), and the sink `Database::events`
/// gives.
#[derive(Clone)]
pub(crate) struct WeakDatabase(Weak<Shared>);

impl WeakDatabase {
    /// The database, while the program holds a handle to it: a handle of
    /// the database's own, which keeps it from going while it is held but
    /// is not counted as the program's.
    pub(crate) fn upgrade(&self) -> Option<Database> {
        let shared = self.0.upgrade()?;
        // The program's last handle is gone, or going: its drop joins the
        // jobs, and keeps the database until it has.
        if shared.program.strong_count() == 0 {
            return None;
        }
        Some(Database(shared, None))
    }
}

/// \[DESIGN\] The sink `Database::events` gives: it tells while the
/// database is open anywhere, the program's last handle gone or not, so
/// what a job meets as its drop waits for it is told too.
struct Forward(WeakDatabase);

impl Diagnostics for Forward {
    fn event(&self, event: &Event) {
        if let Some(shared) = self.0.0.upgrade() {
            Database(shared, None).emit(event);
        }
    }
}

impl Database {
    /// A handle that does not keep the database open.
    pub(crate) fn downgrade(&self) -> WeakDatabase {
        WeakDatabase(Arc::downgrade(&self.0))
    }

    /// A sink that gives each event to the database's sink of the moment,
    /// while the database is open somewhere: for a part of it that tells
    /// of failures it goes on past, as its logs (RULES.md D40, D56).
    pub(crate) fn events(&self) -> Arc<dyn Diagnostics> {
        Arc::new(Forward(self.downgrade()))
    }
}
