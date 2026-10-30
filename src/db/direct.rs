//! \[EXTENSION\] Direct invocation of SQL [ISO/IEC 9075:1992 §20]: a
//! session given statements one at a time, as a client of a server gives
//! them, each a `<direct SQL statement>`: a query, whose rows are read as
//! they are fetched; a `<schema definition>`; or any other statement.

use super::{Completion, Direct, Session};
use crate::ast::AccessMode;
use crate::error::{Code, Result, fail};
use crate::lexer::{Tok, lex};
use crate::parser::{CANCEL, Parser};

impl Session {
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §20.1, §20.2] A `<direct SQL
    /// statement>`: a `<direct select statement: multiple rows>`, a
    /// `<query expression>` with an optional ORDER BY, gives its rows, a
    /// cursor of them read as it is fetched from; a `<schema definition>`,
    /// if it is the session's own, is made in the session's transaction
    /// \[§20.1 GR6b ii, iii\]; any other statement runs as `run` runs it,
    /// and gives its completion. It has no parameters \[§20.1 SR1\].
    ///
    /// # Errors
    ///
    /// As for `open_cursor` or `run`, or for a `<schema definition>` made
    /// in a transaction; SQLCODE -104 for a schema of another authorization
    /// identifier; -917 for one in a READ ONLY transaction.
    pub fn direct(&self, sql: &str) -> Result<Direct> {
        let tokens = lex(sql)?;
        let mut words = tokens.iter().map(|t| &t.tok);
        let (first, second) = (words.next(), words.next());
        // [ISO/IEC 9075:1992 §7.10] a query, a <joined table> of a table's
        // name too, MODULE's: no other statement begins so, but
        // \[EXTENSION\] CANCEL CONNECTION, whose CONNECTION, reserved, no
        // table's name is followed by.
        let named_cancel = if let Some(Tok::Identifier(w)) = first { w == CANCEL } else { false };
        let cancel = named_cancel && matches!(second, Some(Tok::KeyWord("CONNECTION")));
        let joined = (matches!(first, Some(Tok::Identifier(_) | Tok::Introducer(_))) && !cancel)
            || matches!((first, second), (Some(Tok::KeyWord("MODULE")), Some(Tok::Delimiter("."))));
        if joined || matches!(first, Some(Tok::KeyWord("SELECT" | "VALUES" | "TABLE") | Tok::Delimiter("("))) {
            return self.open_cursor(sql).map(Direct::Rows);
        }
        if matches!((first, second), (Some(Tok::KeyWord("CREATE")), Some(Tok::KeyWord("SCHEMA")))) {
            return self.own_schema(sql).map(Direct::Done);
        }
        self.run(sql).map(Direct::Done)
    }

    /// \[DESIGN\] [ISO/IEC 9075:1992 §11.1 AR1] The privileges a `<schema
    /// definition>` needs are implementation-defined: through a session, the
    /// schema, of any name, must be the session's own, its AUTHORIZATION
    /// the session's authorization identifier, written or implicit
    /// \[SR2b\], so a user cannot create objects in another's name.
    /// `Database::schema`, the operator's, may create any.
    fn own_schema(&self, sql: &str) -> Result<Completion> {
        self.emptied(("CREATE SCHEMA", ""));
        let made = self.schema_owned(sql);
        self.diagnosed(made, |done| done.sqlstate())
    }

    /// `own_schema`'s `<schema definition>` `sql`, made.
    fn schema_owned(&self, sql: &str) -> Result<Completion> {
        // [ISO/IEC 9075:1992 §20.1 GR6b iii, iv] An SQL-schema statement,
        // so not of a READ ONLY transaction: the one active, or the next.
        let active = self.inner.borrow().tx.is_some();
        let mode = if active { self.mode.get() } else { self.next.get() };
        if mode.access == AccessMode::ReadOnly {
            return fail(
                Code::TransactionState,
                "[ISO/IEC 9075:1992 §20.1 GR6b iv] a <schema definition> in a READ ONLY transaction".to_owned(),
            );
        }
        let schema = Parser::new(sql)?.of_definition().schema()?;
        if let Some(other) = schema.auth.as_ref().filter(|a| **a != *self.auth()) {
            return fail(
                Code::Privilege,
                format!("[ISO/IEC 9075:1992 §11.1 AR1] {} may create its own schema only, not {other}'s", self.auth()),
            );
        }
        self.schema_in_transaction(sql, &schema.name)
    }
}
