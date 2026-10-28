//! \[EXTENSION\] [ISO/IEC 9075:1992 §21.1] INFORMATION_SCHEMA.USERS: the
//! users (RULES.md D43), an object "associated with INFORMATION_SCHEMA"
//! this implementation adds. The database's owner sees every user; any
//! other, only itself. No verifier is shown. Split from system.rs
//! (RULES.md R-SIZE-2).

use super::{Seen, SystemTable, identifier, text, text_column, yes};
use crate::catalog::ColumnDesc;
use crate::error::Result;
use crate::storage::Row;

/// \[EXTENSION\] INFORMATION_SCHEMA.USERS: each user the session's
/// identifier may see, and whether it owns the database.
pub(super) struct Users;

impl SystemTable for Users {
    fn name(&self) -> &'static str {
        "USERS"
    }

    fn columns(&self) -> Vec<ColumnDesc> {
        vec![identifier("USER_NAME", true), text_column(("IS_OWNER", 3), true)]
    }

    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        let owner = seen.state.users.owner();
        let sees_all = owner == Some(seen.auth);
        let seen_users = seen.state.users.entries().filter(|(name, _)| sees_all || name.as_str() == seen.auth);
        Ok(seen_users.map(|(name, _)| vec![text(name), yes(owner == Some(name.as_str()))]).collect())
    }
}
