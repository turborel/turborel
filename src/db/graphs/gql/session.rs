//! \[EXTENSION\] GQL's session and transaction statements, run as SQL's
//! (graph standard §9, RULES.md D-GQL): SESSION SET SCHEMA is SET SCHEMA,
//! SESSION SET GRAPH sets GQL.GRAPH, SESSION SET TIME ZONE sets
//! SESSION.TIME_ZONE; SESSION RESET takes the session's defaults again;
//! SESSION CLOSE rolls back and resets, the connection staying open to
//! the client. START TRANSACTION sets the next transaction's access mode,
//! as a transaction begins with its first statement; COMMIT and ROLLBACK
//! are SQL's.

use super::GRAPH;
use crate::ast::{GqlReset, GqlSession, GqlTransaction};
use crate::binder::gql::string;
use crate::catalog::identifier;
use crate::db::{Completion, Session};
use crate::error::Result;
use crate::value::Value;

/// The setting of the session's time zone.
const TIME_ZONE: &str = "SESSION.TIME_ZONE";

impl Session {
    /// A GQL session statement.
    ///
    /// # Errors
    ///
    /// As SET SCHEMA's and the settings'.
    pub(super) fn gql_session(&self, s: &GqlSession) -> Result<Completion> {
        match s {
            GqlSession::SetSchema(schema) => self.run(&format!("SET SCHEMA {}", string(schema))),
            GqlSession::SetGraph(name) => {
                let written = match &name.auth {
                    Some(auth) => format!("{}.{}", identifier(auth), identifier(&name.ident)),
                    None => identifier(&name.ident),
                };
                self.set_setting(GRAPH, &Value::Char(written))?;
                Ok(Completion::Success)
            }
            GqlSession::SetTimeZone(zone) => {
                self.set_setting(TIME_ZONE, &Value::Char(zone.as_str().into()))?;
                Ok(Completion::Success)
            }
            GqlSession::Reset(what) => self.gql_reset(*what),
            GqlSession::Close => {
                self.run("ROLLBACK")?;
                self.gql_reset(None)
            }
        }
    }

    /// SESSION RESET of `what`, all if none.
    fn gql_reset(&self, what: Option<GqlReset>) -> Result<Completion> {
        let all = what.is_none();
        if all || what == Some(GqlReset::Schema) {
            self.run(&format!("SET SCHEMA {}", string(&self.auth())))?;
        }
        if all || what == Some(GqlReset::Graph) {
            self.reset_setting(GRAPH)?;
        }
        if all || what == Some(GqlReset::TimeZone) {
            self.reset_setting(TIME_ZONE)?;
        }
        Ok(Completion::Success)
    }

    /// A GQL transaction statement.
    ///
    /// # Errors
    ///
    /// As SET TRANSACTION's, COMMIT's and ROLLBACK's.
    pub(super) fn gql_transaction(&self, t: GqlTransaction) -> Result<Completion> {
        match t {
            GqlTransaction::Start(None) => Ok(Completion::Success),
            GqlTransaction::Start(Some(true)) => self.run("SET TRANSACTION READ ONLY"),
            GqlTransaction::Start(Some(false)) => self.run("SET TRANSACTION READ WRITE"),
            GqlTransaction::Commit => self.run("COMMIT"),
            GqlTransaction::Rollback => self.run("ROLLBACK"),
        }
    }
}
