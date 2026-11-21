//! \[EXTENSION\] Direct invocation of SQL [ISO/IEC 9075:1992 §20]: a
//! session given statements one at a time, as a client of a server gives
//! them, each a `<direct SQL statement>`: a query, whose rows are read as
//! they are fetched; a `<schema definition>`; or any other statement.

use super::area::commands;
use super::{Completion, Direct, Session};
use crate::ast::AccessMode;
use crate::error::{Code, Result, fail};
use crate::graph::algo::SCHEMA;
use crate::lexer::{Tok, lex, lex_head};
use crate::parser::{CANCEL, Parser, begins_word_statement, explained};

/// The tokens that tell a statement apart, at most (`explained`).
const HEAD: usize = 4;

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
        // Told apart by its first tokens, lexed whole only as it is parsed;
        // but a property statement, whose parenthesis may be long, is read
        // all here.
        let mut tokens = lex_head(sql, HEAD)?;
        if let Some(Tok::Identifier(w)) = tokens.first().map(|t| &t.tok)
            && w == SCHEMA
        {
            tokens = lex(sql)?;
        }
        let mut words = tokens.iter().map(|t| &t.tok);
        let (first, second) = (words.next(), words.next());
        // [ISO/IEC 9075:1992 §7.10] a query, a <joined table> of a table's
        // name too, MODULE's: no other statement begins so, but
        // \[EXTENSION\] CANCEL CONNECTION, whose CONNECTION, reserved, no
        // table's name is followed by, and TRUNCATE TABLE and MERGE INTO
        // [ISO/IEC 9075-2:2023 §14.10, §14.12] and INTO PROPERTY alike.
        // \[EXTENSION\] EXPLAIN, no key word either, before a query; and
        // a Cypher query's MATCH [GS §10.3].
        // EXPLAIN ANALYZE, ANALYZE no key word, runs the query.
        if let Some((query, analyze)) = explained(tokens.iter().map(|t| &t.tok)) {
            // The query, after as many spaces as EXPLAIN took, so its
            // errors are placed in the statement as written.
            let at = tokens.get(query).map_or(0, |t| t.start);
            let rest = format!("{}{}", " ".repeat(at), sql.chars().skip(at).collect::<String>());
            let rows = if analyze { self.analyze_rows(&rest) } else { self.explain_rows(&rest) };
            return rows.map(Direct::Rows);
        }
        // [EXTENSION] EXECUTE GQL, and each statement of a GQL session.
        if let Some(done) = self.gql_direct(sql, first)? {
            return Ok(done);
        }
        let named_cancel = if let Some(Tok::Identifier(w)) = first { w == CANCEL } else { false };
        let cancel = (named_cancel && matches!(second, Some(Tok::KeyWord("CONNECTION")))) || begins_word_statement(first, second);
        let property = crate::parser::graph_algo::begins_property_statement(tokens.iter().map(|t| &t.tok));
        let joined = (matches!(first, Some(Tok::Identifier(_) | Tok::Introducer(_))) && !cancel && !property)
            || matches!((first, second), (Some(Tok::KeyWord("MODULE")), Some(Tok::Delimiter("."))));
        if joined || matches!(first, Some(Tok::KeyWord("SELECT" | "VALUES" | "TABLE" | "WITH" | "MATCH") | Tok::Delimiter("("))) {
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
    /// \[SR2b\], or a role enabled for it [ISO/IEC 9075-2:2023 §4.42.4],
    /// so a user cannot create objects in another's name.
    /// `Database::schema`, the operator's, may create any.
    fn own_schema(&self, sql: &str) -> Result<Completion> {
        self.emptied((commands::CREATE_SCHEMA, commands::UNRECOGNIZED));
        let made = self.schema_owned(sql);
        self.diagnosed(made, |done| done.sqlstate())
    }

    /// `own_schema`'s `<schema definition>` `sql`, made.
    fn schema_owned(&self, sql: &str) -> Result<Completion> {
        // [ISO/IEC 9075:1992 §20.1 GR6b iii, iv] An SQL-schema statement,
        // so not of a READ ONLY transaction: the one active, or the next.
        let active = self.inner.borrow().tx.is_some();
        let access = if active { Some(self.mode.get().access) } else { self.next.get().map(|mode| mode.access) };
        if access == Some(AccessMode::ReadOnly) {
            return fail(
                Code::TransactionState,
                "[ISO/IEC 9075:1992 §20.1 GR6b iv] a <schema definition> in a READ ONLY transaction".to_owned(),
            );
        }
        let schema = Parser::for_stored(sql)?.schema()?;
        let (st, authorization) = (self.db.snapshot()?, self.authorization());
        let cell = authorization.of(st.catalog.roles()).cell;
        if let Some(other) = schema.auth.as_ref().filter(|a| !cell.includes(a)) {
            return fail(
                Code::Privilege,
                format!("[ISO/IEC 9075:1992 §11.1 AR1] {} may create its own schema only, not {other}'s", self.auth()),
            );
        }
        self.schema_in_transaction(sql, &schema.name)
    }
}
