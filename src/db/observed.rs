//! \[DESIGN\] What a server observes of a session and its database for the
//! session's client (RULES.md D190, D194): the session's current
//! authorization identifier and the settings it set for itself, which
//! the client is told of as they change, the rows its last statement
//! changed and the identity values it generated, which a completion
//! carries (D197, D198), and the log position the database has applied,
//! which a replica's session waits for before it reads.

use super::{Database, Session};
use crate::error::Result;
use crate::sync::lock_builtin;
use crate::txn::Held;
use crate::value::Value;

impl Session {
    /// [ISO/IEC 9075:1992 §4.30] Its current authorization identifier, as
    /// SET SESSION AUTHORIZATION left it.
    pub fn current_authorization(&self) -> String {
        self.auth().clone()
    }

    /// [ISO/IEC 9075:1992 §18.1 GR1e] The rows its last statement but GET
    /// DIAGNOSTICS inserted, updated, deleted or merged, as ROW_COUNT
    /// gives them; 0 of a statement of another kind (RULES.md D197).
    pub fn row_count(&self) -> usize {
        self.rows_affected()
    }

    /// \[DESIGN\] The values its last INSERT generated for its table's
    /// identity column, in the order of its rows; none of another
    /// statement, or of an INSERT that gave the column its values (RULES.md
    /// D198).
    pub fn generated(&self) -> Vec<Value> {
        self.generated_identities()
    }

    /// The settings it set for itself (RULES.md D36), each its name and its
    /// value, in the order set: none, of a session that set none.
    pub fn own_settings(&self) -> Vec<(String, Value)> {
        let own = self.own.borrow();
        if own.is_empty() {
            return Vec::new();
        }
        let Ok(st) = self.own_snapshot() else { return Vec::new() };
        own.values().filter_map(|(key, value)| Some((st.settings.definition(*key)?.name.clone(), value.clone()))).collect()
    }

    /// \[DESIGN\] Whether its database takes changes now (RULES.md D57,
    /// D190): serving, as a database alone does and a group's primary; not
    /// following a log another replica writes, deposed or retired. A
    /// database closed takes none.
    pub fn takes_changes(&self) -> bool {
        self.own_snapshot().is_ok_and(|st| st.tx.held().is_none())
    }

    /// \[DESIGN\] Whether its current authorization identifier is its
    /// database's owner (RULES.md D43, D190).
    pub fn is_owner(&self) -> bool {
        self.own_snapshot().is_ok_and(|st| st.users.owner() == Some(self.auth().as_str()))
    }
}

impl Database {
    /// \[DESIGN\] The log position whose commit a session of it reads
    /// (RULES.md D194): of a database following its replica group's log,
    /// the last it replayed; of one serving, the end of its log, every
    /// commit in it applied; none, of a database with no log.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database is closed.
    pub fn applied_position(&self) -> Result<Option<u64>> {
        let st = self.snapshot()?;
        if matches!(st.tx.held(), Some(Held::Following)) {
            return Ok(Some(lock_builtin(&st.tx.following).replayed.0));
        }
        Ok(st.log.as_ref().map(|log| log.end().0))
    }
}
