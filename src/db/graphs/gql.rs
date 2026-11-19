//! \[EXTENSION\] GQL requests run by a session (graph standard §9,
//! RULES.md D144): EXECUTE GQL, and each direct statement of a session
//! whose SESSION.LANGUAGE is GQL.
//!
//! A query is lowered to SQL (`binder/gql.rs`) and its rows read as a
//! direct query's are. A procedure that modifies data runs its clauses in
//! order (`gql/modify.rs`): the binding table read so far is read whole,
//! each row's writes made by SQL statements of the element tables, and
//! the table then goes on from its rows. Session and transaction
//! statements are SQL's (`gql/session.rs`). Graph types and the graphs of
//! Mode C are in `gql/graphs.rs`.

mod changes;
mod graphs;
mod identities;
mod inserts;
mod modify;
mod session;
mod writes;

use std::sync::Arc;

use crate::ast::{GqlClause, GqlProcedure, GqlRequest, SqlStatement, TableName};
use crate::binder::gql::{Graph, gql_source, graph_of, lower};
use crate::db::{Completion, Direct, Session, State};
use crate::error::{Code, Result, SqlError};
use crate::parser::Parser;

/// The setting of the language a session's statements are read in.
pub(in crate::db) const LANGUAGE: &str = "SESSION.LANGUAGE";
/// The setting of a GQL session's graph.
pub(in crate::db) const GRAPH: &str = "GQL.GRAPH";

impl Session {
    /// \[EXTENSION\] EXECUTE GQL, run as `run` runs a statement: a query's
    /// rows read and dropped.
    ///
    /// # Errors
    ///
    /// As `gql_request`.
    pub(in crate::db) fn execute_gql(&self, graph: Option<&TableName>, text: &str) -> Result<Completion> {
        match self.gql_request(graph, text)? {
            Direct::Rows(mut rows) => {
                while rows.fetch()?.is_some() {}
                Ok(Completion::Success)
            }
            Direct::Done(done) => Ok(done),
        }
    }

    /// \[EXTENSION\] The direct statement `sql` as GQL, if it is EXECUTE
    /// GQL, or if the session's language is GQL and it is no SET
    /// statement.
    ///
    /// # Errors
    ///
    /// As `gql_request`.
    pub(in crate::db) fn gql_direct(&self, sql: &str, first: Option<&crate::lexer::Tok>) -> Result<Option<Direct>> {
        if first == Some(&crate::lexer::Tok::KeyWord(crate::lexer::kw::EXECUTE)) {
            if let Ok(SqlStatement::ExecuteGql(crate::ast::ExecuteGql { graph, text, .. })) =
                Parser::new(sql).and_then(|mut p| p.sql_statement())
            {
                return self.gql_request(graph.as_ref(), &text).map(Some);
            }
            return Ok(None);
        }
        let set = first == Some(&crate::lexer::Tok::KeyWord(crate::lexer::kw::SET));
        if set || self.language_is_sql()? {
            return Ok(None);
        }
        self.gql_request(None, sql).map(Some)
    }

    /// Whether the session's statements are read as SQL.
    fn language_is_sql(&self) -> Result<bool> {
        Ok(self.setting(LANGUAGE)?.to_string().trim() != "GQL")
    }

    /// \[EXTENSION\] The GQL request `text`, of the graph `graph`, or of
    /// the session's GQL graph if none is named.
    ///
    /// # Errors
    ///
    /// SQLCODE -102 if `text` is no GQL request; as its statements'.
    pub(in crate::db) fn gql_request(&self, graph: Option<&TableName>, text: &str) -> Result<Direct> {
        let source = gql_source(text)?;
        match Parser::new(&source)?.gql_request()? {
            GqlRequest::Session(s) => self.gql_session(&s).map(Direct::Done),
            GqlRequest::Transaction(t) => self.gql_transaction(t).map(Direct::Done),
            GqlRequest::Procedure(p) => {
                if modifies(&p) {
                    return self.gql_modify(graph, &source, &p);
                }
                let st = self.gql_state()?;
                let graphs = |name: Option<&TableName>| self.gql_graph(&st, name.or(graph));
                let lowered = lower(&source, &p, &graphs)?;
                if lowered.vars.is_empty() {
                    let mut rows = self.open_cursor(&lowered.sql)?;
                    while rows.fetch()?.is_some() {}
                    return Ok(Direct::Done(Completion::Success));
                }
                self.open_cursor(&lowered.sql).map(Direct::Rows)
            }
        }
    }

    /// The definitions the session sees.
    pub(in crate::db) fn gql_state(&self) -> Result<Arc<State>> {
        self.definitions(self.db.snapshot()?)
    }

    /// The graph `name`, or the session's GQL graph (GQL.GRAPH) if none.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if none is named and the session has none, or there
    /// is no graph of the name; -104 if the session may not query it.
    pub(in crate::db) fn gql_graph(&self, st: &State, name: Option<&TableName>) -> Result<Graph> {
        let authorization = self.authorization();
        let who = authorization.of(st.catalog.roles()).cell;
        let schema = self.default_schema();
        if let Some(n) = name {
            return graph_of(&*st.catalog, who, &schema, n);
        }
        let setting = self.setting(GRAPH)?.to_string();
        let written = setting.trim();
        if written.is_empty() {
            return Err(SqlError::new(
                Code::SyntaxRule,
                "[EXTENSION] the GQL statement names no graph, by ON GRAPH or USE, and the session has none of SESSION SET GRAPH",
            ));
        }
        let parsed = Parser::new(written)?.gql_graph_name()?;
        graph_of(&*st.catalog, who, &schema, &parsed)
    }
}

/// Whether procedure `p` modifies data.
pub(super) fn modifies(p: &GqlProcedure) -> bool {
    let linear = p.statements.iter().flat_map(|c| std::iter::once(&c.first).chain(c.rest.iter().map(|(_, l)| l)));
    linear.flat_map(|l| &l.clauses).any(|c| match c {
        GqlClause::Insert(_) | GqlClause::Set(_) | GqlClause::Remove(_) | GqlClause::Delete { .. } => true,
        GqlClause::Call(crate::ast::GqlCall::Inline { body, .. }) => modifies(body),
        GqlClause::Match { .. }
        | GqlClause::Filter(_)
        | GqlClause::Let(_)
        | GqlClause::For { .. }
        | GqlClause::Page(_)
        | GqlClause::Call(crate::ast::GqlCall::Named { .. }) => false,
    })
}
