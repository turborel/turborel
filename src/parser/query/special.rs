//! The table references that begin with a word of their own.

use crate::ast::TableReference;
use crate::error::Result;
use crate::parser::Parser;

impl Parser {
    /// A table reference of a word of its own (UNNEST, a table function,
    /// XMLTABLE, a search, JSON_TABLE, GRAPH_TABLE, GQL_TABLE), if one is next.
    pub(super) fn special_table(&mut self) -> Result<Option<TableReference>> {
        let table = if self.at_unnest() {
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
}
