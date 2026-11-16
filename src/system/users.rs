//! \[EXTENSION\] [ISO/IEC 9075:1992 §21.1] INFORMATION_SCHEMA.USERS: the
//! users (RULES.md D43), an object "associated with INFORMATION_SCHEMA"
//! this implementation adds. The database's owner sees every user; any
//! other, only itself. No verifier is shown. CREATE_ROLE tells whether
//! the user may create a role \[§12.4 AR1, IW141\].

use super::{Seen, SystemTable, identifier, text, text_column, yes};
use crate::catalog::ColumnDesc;
use crate::error::Result;
use crate::storage::Row;

/// \[EXTENSION\] INFORMATION_SCHEMA.USERS: each user the session's
/// identifier may see, whether it owns the database, and whether it may
/// create a role.
pub(super) struct Users;

impl SystemTable for Users {
    fn name(&self) -> &'static str {
        "USERS"
    }

    fn columns(&self) -> Vec<ColumnDesc> {
        vec![identifier("USER_NAME", true), text_column(("IS_OWNER", 3), true), text_column(("CREATE_ROLE", 3), true)]
    }

    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        let owner = seen.state.users.owner();
        let sees_all = owner == Some(seen.auth);
        let seen_users = seen.state.users.entries().filter(|(name, _)| sees_all || name.as_str() == seen.auth);
        let users = &seen.state.users;
        Ok(seen_users.map(|(name, _)| vec![text(name), yes(owner == Some(name.as_str())), yes(users.creates_roles(name))]).collect())
    }
}
