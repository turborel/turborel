//! \[EXTENSION\] GQL (ISO/IEC 39075:2024), read from the text EXECUTE GQL,
//! GQL_TABLE or a GQL session gives (graph standard §9): its session and
//! transaction statements, and procedures of linear statements chained by
//! NEXT and combined by set operators.
//!
//! No word of GQL is reserved in SQL text, so each is read as its word
//! where GQL's grammar has it, whether the lexer gives it as a key word or
//! an identifier. MATCH's pattern is read by GPML's parser
//! (`parser/graph.rs`), and checked by its Syntax Rules, as GRAPH_TABLE's
//! is. Each expression keeps the tokens it was read from (`GqlSpan`).
//! The clauses are read in `gql/clauses.rs`, the data-modifying
//! statements in `gql/modify.rs`, and the statements of graph types and
//! graphs SQL text holds in `gql/types.rs`.

mod clauses;
mod modify;
#[cfg(test)]
mod tests;
mod types;

use super::Parser;
use crate::ast::{
    GqlComposite, GqlCondition, GqlExpression, GqlLinear, GqlProcedure, GqlRequest, GqlReset, GqlResult, GqlSession, GqlSetOperator,
    GqlSpan, GqlTransaction,
};
use crate::error::Result;
use crate::lexer::{Tok, delim};

// [ISO/IEC 39075:2024] The words of GQL's statements, none reserved in SQL.
/// SESSION, of the session statements.
pub(crate) const SESSION: &str = "SESSION";
/// GRAPH, of USE's graph and SESSION SET GRAPH.
pub(crate) const GRAPH: &str = "GRAPH";
/// PROPERTY, of PROPERTY GRAPH.
pub(crate) const PROPERTY: &str = "PROPERTY";
/// NEXT, between statements of a procedure.
pub(crate) const NEXT: &str = "NEXT";
/// USE, of a linear statement's graph.
const USE: &str = "USE";
/// OTHERWISE, the set operator.
const OTHERWISE: &str = "OTHERWISE";
/// START, of START TRANSACTION.
const START: &str = "START";
/// RESET, of SESSION RESET.
const RESET: &str = "RESET";
/// CLOSE, of SESSION CLOSE.
const CLOSE: &str = "CLOSE";

impl Parser {
    /// Whether the word `w` is next, a key word or an identifier.
    pub(super) fn at_gql(&self, w: &str) -> bool {
        self.gql_at(0, w)
    }

    /// Whether the word `w` is `n` tokens ahead.
    pub(super) fn gql_at(&self, n: usize, w: &str) -> bool {
        match self.peek_at(n) {
            Tok::KeyWord(k) => *k == w,
            Tok::Identifier(s) => s == w,
            Tok::ExactNumeric(_)
            | Tok::ApproximateNumeric(_)
            | Tok::CharacterString(_)
            | Tok::BinaryString(_)
            | Tok::Introducer(_)
            | Tok::LargeObjectLength(_)
            | Tok::Delimiter(_)
            | Tok::End => false,
        }
    }

    /// Takes the word `w` if it is next.
    pub(super) fn accept_gql(&mut self, w: &str) -> bool {
        let next = self.at_gql(w);
        if next {
            self.advance();
        }
        next
    }

    /// Takes the word `w`.
    ///
    /// # Errors
    ///
    /// SQLCODE -102 if it is not next.
    pub(super) fn expect_gql(&mut self, w: &str) -> Result<()> {
        if self.accept_gql(w) { Ok(()) } else { Err(self.syntax(format!("[EXTENSION] expected {w}"))) }
    }

    /// The token number of the token at hand.
    pub(super) fn token_number(&self) -> usize {
        self.pos
    }

    /// \[EXTENSION\] A GQL request, the whole text.
    ///
    /// # Errors
    ///
    /// SQLCODE -102 if the text is not one; -103 if a pattern breaks a
    /// Syntax Rule of GPML.
    pub fn gql_request(&mut self) -> Result<GqlRequest> {
        self.graph = self.graph.saturating_add(1);
        let read = self.gql_request_body();
        self.graph = self.graph.saturating_sub(1);
        let request = read?;
        self.accept_delim(delim::SEMICOLON);
        if *self.peek() != Tok::End {
            return Err(self.syntax("[EXTENSION] expected the end of the GQL request"));
        }
        Ok(request)
    }

    /// A session statement, a transaction statement, or a procedure.
    fn gql_request_body(&mut self) -> Result<GqlRequest> {
        if self.accept_gql(SESSION) {
            return self.gql_session().map(GqlRequest::Session);
        }
        if self.at_gql(START) || self.at_gql(crate::lexer::kw::COMMIT) || self.at_gql(crate::lexer::kw::ROLLBACK) {
            return self.gql_transaction().map(GqlRequest::Transaction);
        }
        self.gql_procedure().map(GqlRequest::Procedure)
    }

    /// The rest of a session statement, after SESSION.
    fn gql_session(&mut self) -> Result<GqlSession> {
        if self.accept_gql(CLOSE) {
            return Ok(GqlSession::Close);
        }
        if self.accept_gql(RESET) {
            self.accept_gql(crate::lexer::kw::ALL);
            return Ok(GqlSession::Reset(self.gql_reset_target()));
        }
        self.expect_gql(crate::lexer::kw::SET)?;
        if self.accept_gql(crate::lexer::kw::SCHEMA) {
            return self.identifier().map(GqlSession::SetSchema);
        }
        if self.accept_gql(crate::lexer::kw::TIME) {
            self.expect_gql(crate::lexer::kw::ZONE)?;
            return self.character_string_literal("a time zone").map(GqlSession::SetTimeZone);
        }
        self.accept_gql(PROPERTY);
        self.expect_gql(GRAPH)?;
        self.table_name().map(GqlSession::SetGraph)
    }

    /// What SESSION RESET names, if anything.
    fn gql_reset_target(&mut self) -> Option<GqlReset> {
        if self.accept_gql(crate::lexer::kw::SCHEMA) {
            return Some(GqlReset::Schema);
        }
        if self.accept_gql(crate::lexer::kw::TIME) {
            self.accept_gql(crate::lexer::kw::ZONE);
            return Some(GqlReset::TimeZone);
        }
        self.accept_gql(PROPERTY);
        self.accept_gql(GRAPH).then_some(GqlReset::Graph)
    }

    /// START TRANSACTION \[READ ONLY | READ WRITE\], COMMIT or ROLLBACK
    /// \[WORK\].
    fn gql_transaction(&mut self) -> Result<GqlTransaction> {
        if self.accept_gql(crate::lexer::kw::COMMIT) {
            self.accept_gql(crate::lexer::kw::WORK);
            return Ok(GqlTransaction::Commit);
        }
        if self.accept_gql(crate::lexer::kw::ROLLBACK) {
            self.accept_gql(crate::lexer::kw::WORK);
            return Ok(GqlTransaction::Rollback);
        }
        self.expect_gql(START)?;
        self.expect_gql(crate::lexer::kw::TRANSACTION)?;
        if !self.accept_gql(crate::lexer::kw::READ) {
            return Ok(GqlTransaction::Start(None));
        }
        if self.accept_gql(crate::lexer::kw::ONLY) {
            return Ok(GqlTransaction::Start(Some(true)));
        }
        self.expect_gql(crate::lexer::kw::WRITE)?;
        Ok(GqlTransaction::Start(Some(false)))
    }

    /// A procedure: composite statements, NEXT between each two.
    pub(super) fn gql_procedure(&mut self) -> Result<GqlProcedure> {
        let mut statements = vec![self.gql_composite()?];
        while self.accept_gql(NEXT) {
            statements.push(self.gql_composite()?);
        }
        Ok(GqlProcedure { statements })
    }

    /// Linear statements and the set operators between them.
    fn gql_composite(&mut self) -> Result<GqlComposite> {
        let first = self.gql_linear()?;
        let mut rest = Vec::new();
        while let Some(op) = self.gql_set_operator() {
            rest.push((op, self.gql_linear()?));
        }
        Ok(GqlComposite { first, rest })
    }

    /// UNION, EXCEPT or INTERSECT \[ALL | DISTINCT\], or OTHERWISE, if
    /// next.
    fn gql_set_operator(&mut self) -> Option<GqlSetOperator> {
        if self.accept_gql(OTHERWISE) {
            return Some(GqlSetOperator::Otherwise);
        }
        let make: fn(bool) -> GqlSetOperator = if self.accept_gql(crate::lexer::kw::UNION) {
            GqlSetOperator::Union
        } else if self.accept_gql(crate::lexer::kw::EXCEPT) {
            GqlSetOperator::Except
        } else if self.accept_gql(crate::lexer::kw::INTERSECT) {
            GqlSetOperator::Intersect
        } else {
            return None;
        };
        let all = self.accept_gql(crate::lexer::kw::ALL);
        if !all {
            self.accept_gql(crate::lexer::kw::DISTINCT);
        }
        Some(make(all))
    }

    /// \[USE `<graph>`\] clauses, then RETURN, FINISH or nothing.
    fn gql_linear(&mut self) -> Result<GqlLinear> {
        let at = self.here();
        let graph = if self.accept_gql(USE) { Some(self.table_name()?) } else { None };
        let mut clauses = Vec::new();
        while let Some(clause) = self.gql_clause()? {
            clauses.push(clause);
        }
        let result = self.gql_result()?;
        if clauses.is_empty() && matches!(result, GqlResult::None) {
            return Err(self.syntax("[EXTENSION] expected a GQL statement"));
        }
        Ok(GqlLinear { graph, clauses, result, at })
    }

    /// A value expression and its tokens.
    /// \[EXTENSION\] A graph's name, read whole: GQL.GRAPH's value.
    ///
    /// # Errors
    ///
    /// SQLCODE -102 if it is none.
    pub(crate) fn gql_graph_name(&mut self) -> Result<crate::ast::TableName> {
        let name = self.table_name()?;
        self.end()?;
        Ok(name)
    }

    /// \[EXTENSION\] The `<query expression>` a GQL query was lowered
    /// to (`binder/gql.rs`), read whole.
    ///
    /// # Errors
    ///
    /// SQLCODE -102 if it is none.
    pub(crate) fn gql_lowered(sql: &str) -> Result<crate::ast::QueryExpression> {
        let mut p = Parser::new(sql)?;
        let q = p.query_expression()?;
        p.end()?;
        Ok(q)
    }

    pub(super) fn gql_expression(&mut self) -> Result<GqlExpression> {
        let start = self.token_number();
        let value = self.value_expression()?;
        Ok(GqlExpression { value, span: GqlSpan { start, end: self.token_number() } })
    }

    /// A search condition and its tokens.
    pub(super) fn gql_condition(&mut self) -> Result<GqlCondition> {
        let start = self.token_number();
        let condition = self.search_condition()?;
        Ok(GqlCondition { condition, span: GqlSpan { start, end: self.token_number() } })
    }
}
