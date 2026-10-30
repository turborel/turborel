//! \[EXTENSION\] The connections of a server that serves the database
//! (RULES.md D42), as SQL reaches them: END CONNECTION and CANCEL
//! CONNECTION (`parser/served.rs`), by the database's owner. The database
//! knows nothing of a server: one serving it registers what ends and
//! cancels its connections, `ServedConnections`, an extension point; with
//! none registered, no connection is served.

use std::sync::{Arc, PoisonError};

use super::{Completion, Database, Session};
use crate::ast::ServedStatement;
use crate::diagnostics::{Event, Extension};
use crate::error::{Code, Result, SqlError, SqlState, fail};

/// \[EXTENSION\] What ends a server's connections, or cancels what they
/// run, by their IDs in INFORMATION_SCHEMA.CONNECTIONS: an extension point
/// (RULES.md D42), registered by the server that serves the database.
///
/// Contract (RULES.md R-SAFE-3): `end` and `cancel` return at once, false
/// if no connection of `id` is served, waiting for nothing: what they ask
/// is done on the connection's own time. `end` cancels what the connection
/// runs, then closes it, its session rolled back; `cancel` cancels the
/// statement it runs, if one runs (RULES.md D44), and its session goes on.
/// Neither calls back into the database (R-ERR-4).
pub trait ServedConnections: Send + Sync {
    /// Its name, for diagnostics.
    fn name(&self) -> &'static str;
    /// Ends the connection `id`; false if none of `id` is served.
    fn end(&self, id: u64) -> bool;
    /// Cancels what the connection `id` runs; false if none of `id` is
    /// served.
    fn cancel(&self, id: u64) -> bool;
}

impl Database {
    /// \[EXTENSION\] Sets what ends and cancels the connections of the
    /// server that serves the database, in place of any set before.
    pub fn set_served_connections(&self, served: Box<dyn ServedConnections>) {
        let name = served.name().to_owned();
        *self.served_slot().write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::from(served));
        self.emit(&Event::Registered { kind: Extension::ServedConnections, name });
    }

    /// What ends and cancels the served connections, if it was set.
    fn served_connections(&self) -> Option<Arc<dyn ServedConnections>> {
        self.served_slot().read().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl Session {
    /// \[EXTENSION\] Runs `stmt`: by the database's owner only, in a
    /// transaction or not, as it changes no definition and is not logged.
    ///
    /// # Errors
    ///
    /// SQLCODE -104 if the session's identifier is not the owner's;
    /// SQLSTATE 08003 if no connection of the ID is served.
    pub(super) fn served_statement(&self, stmt: &ServedStatement) -> Result<Completion> {
        let (what, (id, at)) = match stmt {
            ServedStatement::End { id } => ("END CONNECTION", *id),
            ServedStatement::Cancel { id } => ("CANCEL CONNECTION", *id),
        };
        if self.db.owner()?.as_deref() != Some(self.auth().as_str()) {
            return fail(Code::Privilege, format!("[EXTENSION] only the database's owner runs {what}, and {} is not it", self.auth()));
        }
        let served = self.db.served_connections();
        let done = served.is_some_and(|s| match stmt {
            ServedStatement::End { .. } => s.end(id),
            ServedStatement::Cancel { .. } => s.cancel(id),
        });
        if !done {
            let msg = format!("[EXTENSION] no connection {id} is served");
            return Err(SqlError::at(Code::Connection, at, msg).with_state(SqlState::CONNECTION_DOES_NOT_EXIST));
        }
        Ok(Completion::Success)
    }
}
