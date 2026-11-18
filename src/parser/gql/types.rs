//! \[EXTENSION\] The statements of GQL graphs SQL text holds (graph
//! standard §4.3, §9): EXECUTE GQL, the `<table primary>` GQL_TABLE, CREATE
//! and DROP GRAPH TYPE, and CREATE and DROP GRAPH of Mode C, typed or open.
//!
//! GRAPH is no key word, and CREATE GRAPH is told from CREATE PROPERTY
//! GRAPH, Mode A's, by the word after GRAPH's place.

use super::super::Parser;
use super::{GRAPH, PROPERTY};
use crate::ast::{
    DropObject, EdgeTypeDefinition, ExecuteGql, GqlGraphDefinition, GqlTable, GraphTypeDefinition, GraphTyping, NodeTypeDefinition,
    PropertyTypeDefinition, SchemaStatement, SqlStatement, TableReference,
};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};
use crate::value::DataType;

// [Graph standard §4.3, §9] The words of these statements.
/// GQL_TABLE, the table's name.
pub const GQL_TABLE: &str = "GQL_TABLE";
/// GQL, of EXECUTE GQL.
pub(crate) const GQL: &str = "GQL";
/// TYPE, of GRAPH TYPE, NODE TYPE and EDGE TYPE.
const TYPE: &str = "TYPE";
/// TYPED, of CREATE GRAPH.
const TYPED: &str = "TYPED";
/// ANY, of an open graph.
const ANY: &str = "ANY";
/// NODE, of a node type.
const NODE: &str = "NODE";
/// EDGE, of an edge type.
const EDGE: &str = "EDGE";
/// DIRECTED, of an edge type.
const DIRECTED: &str = "DIRECTED";
/// UNDIRECTED, of an edge type.
const UNDIRECTED: &str = "UNDIRECTED";
/// CONNECTING, of an edge type's endpoints.
const CONNECTING: &str = "CONNECTING";
/// LABEL, of a node or edge type's labels.
const LABEL: &str = "LABEL";
/// LABELS, of a node or edge type's labels.
const LABELS: &str = "LABELS";
/// IF, of IF EXISTS and IF NOT EXISTS.
const IF: &str = "IF";

/// [ISO/IEC 39075:2024 §18.9] GQL's value types that are no SQL type's
/// name, each with the SQL type it is held as.
const GQL_TYPES: [(&str, &str); 9] = [
    ("STRING", "CHARACTER VARYING(32767)"),
    ("BOOL", "BOOLEAN"),
    ("INT8", "SMALLINT"),
    ("INT16", "SMALLINT"),
    ("INT32", "INTEGER"),
    ("INT64", "BIGINT"),
    ("FLOAT32", "REAL"),
    ("FLOAT64", "DOUBLE PRECISION"),
    ("DATETIME", "TIMESTAMP"),
];

impl Parser {
    /// A statement of GQL SQL text holds, if one begins here.
    ///
    /// # Errors
    ///
    /// SQLCODE -102 if it does not conform to its Format.
    pub(in crate::parser) fn gql_sql_statement(&mut self) -> Result<Option<SqlStatement>> {
        let start = self.here();
        if self.is_kw(kw::EXECUTE) && self.gql_at(1, GQL) {
            return self.execute_gql().map(Some);
        }
        let graph = self.gql_at(1, GRAPH) && !self.gql_at(1, PROPERTY);
        if self.is_kw(kw::DROP) && graph {
            return self.drop_gql_graph().map(Some);
        }
        if !(self.is_kw(kw::CREATE) && graph) {
            return Ok(None);
        }
        self.advance();
        self.advance();
        let statement = if self.accept_gql(TYPE) {
            SchemaStatement::GraphType(Box::new(self.graph_type_definition()?))
        } else {
            SchemaStatement::GqlGraph(self.gql_graph_definition()?)
        };
        Ok(Some(SqlStatement::Schema { statement, text: self.text_from(start) }))
    }

    /// EXECUTE GQL \[ON GRAPH `<graph name>`\] `<character string
    /// literal>`.
    fn execute_gql(&mut self) -> Result<SqlStatement> {
        let at = self.here();
        self.advance();
        self.advance();
        let graph = if self.accept_kw(kw::ON) {
            self.accept_gql(PROPERTY);
            self.expect_gql(GRAPH)?;
            Some(self.table_name()?)
        } else {
            None
        };
        let text = self.character_string_literal("GQL text")?;
        Ok(SqlStatement::ExecuteGql(ExecuteGql { graph, text, at }))
    }

    /// \[IF NOT EXISTS\], if written.
    fn if_not_exists(&mut self) -> bool {
        let written = self.at_gql(IF) && self.gql_at(1, kw::NOT) && self.gql_at(2, kw::EXISTS);
        if written {
            self.advance();
            self.advance();
            self.advance();
        }
        written
    }

    /// DROP GRAPH \[TYPE\] \[IF EXISTS\] `<name>` \[RESTRICT | CASCADE\].
    fn drop_gql_graph(&mut self) -> Result<SqlStatement> {
        self.advance();
        self.advance();
        let of_type = self.accept_gql(TYPE);
        let if_exists = self.at_gql(IF) && self.gql_at(1, kw::EXISTS);
        if if_exists {
            self.advance();
            self.advance();
        }
        let name = self.table_name()?;
        let object = if of_type { DropObject::GraphType(name, if_exists) } else { DropObject::GqlGraph(name, if_exists) };
        Ok(SqlStatement::Drop { object, behavior: self.optional_behavior()? })
    }

    /// The rest of CREATE GRAPH: \[IF NOT EXISTS\] `<name>` \[{TYPED | ::}
    /// `<graph type name>` | ANY \[PROPERTY GRAPH\]\].
    fn gql_graph_definition(&mut self) -> Result<GqlGraphDefinition> {
        let if_not_exists = self.if_not_exists();
        let name = self.table_name()?;
        let typing = if self.accept_gql(TYPED) || self.accept_delim(delim::DOUBLE_COLON) {
            GraphTyping::Typed(self.table_name()?)
        } else {
            if self.accept_gql(ANY) && self.accept_gql(PROPERTY) {
                self.expect_gql(GRAPH)?;
            }
            GraphTyping::Open
        };
        Ok(GqlGraphDefinition { name, if_not_exists, typing })
    }

    /// The rest of CREATE GRAPH TYPE: \[IF NOT EXISTS\] `<name>` \[AS\] {
    /// `<element type>` \[, ...\] }.
    ///
    /// # Errors
    ///
    /// SQLCODE -102 if it does not conform to its Format.
    pub(crate) fn graph_type_definition(&mut self) -> Result<GraphTypeDefinition> {
        let if_not_exists = self.if_not_exists();
        let name = self.table_name()?;
        self.accept_kw(kw::AS);
        self.expect_delim(delim::LEFT_BRACE)?;
        let (mut nodes, mut edges) = (Vec::new(), Vec::new());
        loop {
            if self.at_gql(NODE) {
                nodes.push(self.node_type()?);
            } else {
                edges.push(self.edge_type()?);
            }
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        self.expect_delim(delim::RIGHT_BRACE)?;
        Ok(GraphTypeDefinition { name, if_not_exists, nodes, edges })
    }

    /// NODE \[TYPE\] `<name>` \[labels\] \[properties\].
    fn node_type(&mut self) -> Result<NodeTypeDefinition> {
        let at = self.here();
        self.advance();
        self.accept_gql(TYPE);
        let name = self.identifier()?;
        let labels = self.type_labels(&name)?;
        Ok(NodeTypeDefinition { name, labels, properties: self.property_types()?, at })
    }

    /// \[DIRECTED | UNDIRECTED\] EDGE \[TYPE\] `<name>` \[labels\]
    /// \[properties\] CONNECTING ( `<node type>` {-> | ~} `<node type>` ).
    fn edge_type(&mut self) -> Result<EdgeTypeDefinition> {
        let at = self.here();
        let directed = !self.accept_gql(UNDIRECTED);
        self.accept_gql(DIRECTED);
        self.expect_gql(EDGE)?;
        self.accept_gql(TYPE);
        let name = self.identifier()?;
        let labels = self.type_labels(&name)?;
        let properties = self.property_types()?;
        self.expect_gql(CONNECTING)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let source = self.identifier()?;
        if directed {
            self.expect_delim(delim::RIGHT_ARROW)?;
        } else {
            self.expect_delim(delim::TILDE)?;
        }
        let destination = self.identifier()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(EdgeTypeDefinition { name, labels, properties, directed, source, destination, at })
    }

    /// \[{LABEL | LABELS | :} `<label>` \[& ...\]\]: the labels written,
    /// else the type's name alone.
    fn type_labels(&mut self, name: &str) -> Result<Vec<String>> {
        if !(self.accept_gql(LABEL) || self.accept_gql(LABELS) || self.accept_delim(delim::COLON)) {
            return Ok(vec![name.to_owned()]);
        }
        let mut labels = vec![self.identifier()?];
        while self.accept_delim(delim::AMPERSAND) {
            labels.push(self.identifier()?);
        }
        Ok(labels)
    }

    /// \[(\] { `<property type>` \[, ...\] } \[)\], or nothing.
    fn property_types(&mut self) -> Result<Vec<PropertyTypeDefinition>> {
        let parenthesized = self.is_delim(delim::LEFT_PAREN) && *self.peek_at(1) == Tok::Delimiter(delim::LEFT_BRACE);
        if parenthesized {
            self.advance();
        }
        if !self.accept_delim(delim::LEFT_BRACE) {
            return Ok(Vec::new());
        }
        let mut properties = Vec::new();
        while !self.accept_delim(delim::RIGHT_BRACE) {
            let name = self.identifier()?;
            let data_type = self.gql_value_type()?;
            let not_null = self.is_kw(kw::NOT) && self.next_is_kw(kw::NULL);
            if not_null {
                self.advance();
                self.advance();
            }
            properties.push(PropertyTypeDefinition { name, data_type, not_null });
            if !self.is_delim(delim::RIGHT_BRACE) {
                self.expect_delim(delim::COMMA)?;
            }
        }
        if parenthesized {
            self.expect_delim(delim::RIGHT_PAREN)?;
        }
        Ok(properties)
    }

    /// A GQL value type: one of `GQL_TYPES`, or an SQL `<data type>`.
    fn gql_value_type(&mut self) -> Result<DataType> {
        let held = GQL_TYPES.iter().find(|(word, _)| *self.peek() == Tok::Identifier((*word).to_owned()));
        let Some((_, sql)) = held else {
            return self.data_type();
        };
        self.advance();
        Parser::new(sql)?.data_type()
    }

    /// Whether a GQL_TABLE begins here: GQL_TABLE before "(".
    pub(in crate::parser) fn at_gql_table(&self) -> bool {
        self.at_json(GQL_TABLE)
    }

    /// GQL_TABLE ( `<graph name>` , `<character string literal>` ) \[\[AS\]
    /// `<correlation name>` \[( `<derived column list>` )\]\].
    pub(in crate::parser) fn gql_table(&mut self) -> Result<TableReference> {
        let at = self.here();
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        let graph = self.table_name()?;
        self.expect_delim(delim::COMMA)?;
        let text = self.character_string_literal("GQL text")?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        let correlation = self.correlation()?;
        let columns = if correlation.is_some() { self.derived_columns()? } else { None };
        Ok(TableReference::Gql(Box::new(GqlTable { graph, text, correlation, columns, at })))
    }
}
