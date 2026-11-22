//! The table references that begin with a word of their own.

use crate::ast::{DerivedTable, TableReference};
use crate::error::Result;
use crate::lexer::{Tok, delim};
use crate::parser::Parser;

/// [ISO/IEC 9075-2:2023 §7.6] LATERAL, before the `<table subquery>` of a
/// `<lateral derived table>`.
const LATERAL: &str = "LATERAL";

impl Parser {
    /// A table reference of a word of its own (LATERAL, UNNEST, a table
    /// function, XMLTABLE, a search, JSON_TABLE, GRAPH_TABLE, GQL_TABLE), if
    /// one is next.
    pub(super) fn special_table(&mut self) -> Result<Option<TableReference>> {
        let table = if self.is_word(LATERAL) && matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN)) {
            self.lateral_derived_table()?
        } else if self.at_unnest() {
            self.collection_derived_table()?
        } else if self.at_chunk_table() {
            self.chunk_table()?
        } else if self.at_xml_table() {
            self.xml_table()?
        } else if self.at_ptf_table() {
            self.ptf_table()?
        } else if self.at_vector_search() {
            self.vector_search()?
        } else if self.at_graph_algorithm() {
            self.graph_algorithm_table()?
        } else if self.at_json_table() {
            self.json_table()?
        } else if self.at_graph_table() {
            self.graph_table()?
        } else if self.at_hybrid_search() {
            self.hybrid_search()?
        } else if self.at_gql_table() {
            self.gql_table()?
        } else {
            return Ok(None);
        };
        Ok(Some(table))
    }

    /// [ISO/IEC 9075-2:2023 §7.6] `<lateral derived table>` ::= LATERAL
    /// `<table subquery>`, then \[AS\] `<correlation name>` \[(
    /// `<derived column list>` )\], the correlation name not optional.
    fn lateral_derived_table(&mut self) -> Result<TableReference> {
        self.advance();
        if !self.at_subquery() {
            return Err(self.syntax("[ISO/IEC 9075-2:2023 §7.6 F] LATERAL is followed by a <table subquery>"));
        }
        let query = Box::new(self.subquery()?);
        let correlation = self
            .correlation()?
            .ok_or_else(|| self.syntax("[ISO/IEC 9075-2:2023 §7.6 F] a <lateral derived table> has a correlation name"))?;
        let columns = self.derived_columns()?;
        Ok(TableReference::Derived(DerivedTable { query, correlation, columns, lateral: true }))
    }
}
