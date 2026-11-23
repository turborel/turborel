//! \[DESIGN\] Whether a server takes changes (RULES.md D201, D57). A
//! server alone does, and of a replicated group its primary. A replica
//! following the group's log refuses every change (SQLCODE -917). A
//! replica shows its role in INFORMATION_SCHEMA.REPLICAS, its own row
//! `IS_SELF = 'YES'`, to the database's owner alone; a server alone has no
//! such table. So a session of the owner learns either role, and another
//! session learns that a server alone takes changes, the role of a
//! replica being unknown to it.
//!
//! A server that grants `status` tells every session whether it takes
//! changes, SERVER_READ_ONLY, as the session begins and again after each
//! statement (RULES.md D190), so `role` reads it with no query and sees
//! a replica's role whoever the session's user is. The query is the
//! fallback, for a server that does not grant it.

use std::io::{Read, Write};

use crate::connection::{Connection, Outcome, Result};

/// \[DESIGN\] The query of a server's own role in its group.
pub const ROLE_QUERY: &str = "SELECT ROLE FROM INFORMATION_SCHEMA.REPLICAS WHERE IS_SELF = 'YES'";
/// \[DESIGN\] The session status of whether the server takes no changes.
pub const SERVER_READ_ONLY: &str = "SERVER_READ_ONLY";
/// \[DESIGN\] The status's word for a server that takes none.
const YES: &str = "YES";
/// \[DESIGN\] The role REPLICAS shows of the group's primary.
const PRIMARY: &str = "PRIMARY";
/// \[DESIGN\] The SQLSTATE classes of a table that does not exist, as a
/// server alone answers `ROLE_QUERY`: of a syntax error or access rule
/// violation, of a direct SQL statement or another.
const NO_SUCH_TABLE_CLASSES: [&str; 2] = ["2A", "42"];

/// What a server is, to a connection's session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// It takes changes.
    ReadWrite,
    /// It takes none: a replica following its group.
    ReadOnly,
    /// A replica whose role the session cannot see.
    Unknown,
}

impl Role {
    /// It, in words.
    pub fn describe(self) -> &'static str {
        match self {
            Role::ReadWrite => "read-write",
            Role::ReadOnly => "read-only",
            Role::Unknown => "of a role this session cannot see",
        }
    }
}

/// The role `answer`, the outcome of `ROLE_QUERY`, shows.
///
/// # Errors
///
/// The query's failure, unless it is of a server alone, which has no
/// REPLICAS.
pub(crate) fn role_of(answer: Result<Outcome>) -> Result<Role> {
    let rows = match answer {
        Ok(Outcome::Rows { rows, .. }) => rows,
        Ok(Outcome::Done(_)) => Vec::new(),
        Err(failure) => {
            if NO_SUCH_TABLE_CLASSES.iter().any(|class| failure.sqlstate.starts_with(class)) {
                return Ok(Role::ReadWrite);
            }
            return Err(failure);
        }
    };
    let Some(row) = rows.first() else { return Ok(Role::Unknown) };
    let primary = row.first().is_some_and(|v| v.to_string().trim_end() == PRIMARY);
    Ok(if primary { Role::ReadWrite } else { Role::ReadOnly })
}

/// The role a session status of SERVER_READ_ONLY of `told` shows.
pub(crate) fn role_told(told: &str) -> Role {
    if told == YES { Role::ReadOnly } else { Role::ReadWrite }
}

/// The role of `connection`'s server, as its session sees it: as its
/// session status last told it, if the server tells it; else asked as a
/// statement that is its own transaction, so it leaves none open.
///
/// # Errors
///
/// A connection failure; the query's, but of a server alone.
pub fn role<S: Read + Write>(connection: &mut Connection<S>) -> Result<Role> {
    if let Some(told) = connection.setting(SERVER_READ_ONLY) {
        return Ok(role_told(told));
    }
    let autocommit = connection.autocommit();
    connection.set_autocommit(true);
    let answer = connection.query(ROLE_QUERY);
    connection.set_autocommit(autocommit);
    role_of(answer)
}

#[cfg(test)]
mod tests {
    use turborel::error::{Code, SqlError, SqlState};
    use turborel_protocol::{Completed, Failure};

    use turborel::value::Value;

    use super::*;

    /// The outcome of `ROLE_QUERY` of one row, of `role`.
    fn shown(role: &str) -> Outcome {
        Outcome::Rows { columns: Vec::new(), rows: vec![vec![Value::Char(role.into())]] }
    }

    /// Verifies: [DESIGN] RULES.md D201, D57: a server alone, which has
    /// no REPLICAS (class 2A or 42), takes changes; of a replica, PRIMARY is
    /// read-write, BACKUP or UNKNOWN read-only; no row, as a session not
    /// the owner's sees, is unknown; another failure is the query's.
    #[test]
    fn a_servers_role_is_read_from_replicas() {
        let missing = Failure::from(&SqlError::new(Code::SyntaxRule, "no table"));
        assert_eq!(role_of(Err(missing)).unwrap(), Role::ReadWrite);
        assert_eq!(role_of(Ok(shown("PRIMARY"))).unwrap(), Role::ReadWrite);
        assert_eq!(role_of(Ok(shown("BACKUP "))).unwrap(), Role::ReadOnly);
        assert_eq!(role_of(Ok(shown("UNKNOWN"))).unwrap(), Role::ReadOnly);
        assert_eq!(role_of(Ok(Outcome::Rows { columns: Vec::new(), rows: Vec::new() })).unwrap(), Role::Unknown);
        let done = Completed::of(0, "00000");
        assert_eq!(role_of(Ok(Outcome::Done(done))).unwrap(), Role::Unknown);
        let lost = Failure::from(&SqlError::new(Code::Connection, "lost").with_state(SqlState::CONNECTION_FAILURE));
        assert_eq!(role_of(Err(lost)).unwrap_err().sqlstate, "08006");
        assert_eq!(Role::Unknown.describe(), "of a role this session cannot see");
        assert_eq!((role_told("YES"), role_told("NO")), (Role::ReadOnly, Role::ReadWrite));
    }
}
