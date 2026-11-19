//! \[EXTENSION\] The label and reachability indexes of a property graph
//! (RULES.md D149, D150): CREATE LABEL INDEX and CREATE REACHABILITY
//! INDEX, each the CREATE INDEX of its method, as CREATE ADJACENCY INDEX
//! is (`graph_ddl.rs`).

use super::{EDGE, GRAPH, INDEX, LABEL, VERTEX};
use crate::ast::{IndexStatement, LabelIndex, ReachabilityIndex, SqlStatement, TableName};
use crate::error::Result;
use crate::lexer::{Tok, kw};
use crate::parser::Parser;

/// The `<non-reserved word>` REACHABILITY.
pub(in crate::parser) const REACHABILITY: &str = "REACHABILITY";

impl Parser {
    /// Whether CREATE LABEL INDEX begins here, after CREATE.
    pub(in crate::parser) fn label_index_next(&self) -> bool {
        self.is_word(LABEL) && matches!(self.peek_at(1), Tok::Identifier(s) if s == INDEX)
    }

    /// INDEX \[`<index name>`\] ON GRAPH `<graph name>`, after the
    /// method's word.
    fn graph_index_head(&mut self) -> Result<(Option<(String, usize)>, TableName)> {
        self.expect_word(INDEX)?;
        let name = if self.is_kw(kw::ON) { None } else { Some(self.label_named()?) };
        self.expect_kw(kw::ON)?;
        self.expect_word(GRAPH)?;
        Ok((name, self.table_name()?))
    }

    /// \[EXTENSION\] CREATE LABEL INDEX \[`<index name>`\] ON GRAPH
    /// `<graph name>` \[{VERTEX | EDGE} TABLE `<alias>`\], after CREATE.
    ///
    /// # Errors
    ///
    /// SQLCODE -102 if it does not conform to the Format.
    pub(in crate::parser) fn label_index(&mut self) -> Result<SqlStatement> {
        self.expect_word(LABEL)?;
        let (name, graph) = self.graph_index_head()?;
        let edge = self.accept_word(EDGE);
        let table = if edge || self.accept_word(VERTEX) {
            self.expect_kw(kw::TABLE)?;
            let (alias, at) = self.label_named()?;
            Some((alias, edge, at))
        } else {
            None
        };
        Ok(SqlStatement::Index(IndexStatement::Labels(Box::new(LabelIndex { name, graph, table }))))
    }

    /// \[EXTENSION\] CREATE REACHABILITY INDEX \[`<index name>`\] ON GRAPH
    /// `<graph name>` EDGE TABLE `<alias>`, after CREATE REACHABILITY.
    ///
    /// # Errors
    ///
    /// SQLCODE -102 if it does not conform to the Format.
    pub(in crate::parser) fn reachability_index(&mut self) -> Result<SqlStatement> {
        let (name, graph) = self.graph_index_head()?;
        self.expect_word(EDGE)?;
        self.expect_kw(kw::TABLE)?;
        let edge = self.label_named()?;
        Ok(SqlStatement::Index(IndexStatement::Reachability(Box::new(ReachabilityIndex { name, graph, edge }))))
    }
}
