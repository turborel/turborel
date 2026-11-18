//! \[EXTENSION\] The statements that define property graphs over tables
//! (RULES.md D-GRAPH-CATALOG), Mode A of the property graph
//! implementation standard (§4.1), written in the notation of SQL/PGQ:
//!
//! ```text
//! <create property graph statement> ::=
//!     CREATE [ OR REPLACE ] PROPERTY GRAPH [ IF NOT EXISTS ] <property graph name>
//!       VERTEX TABLES ( <element table definition> [ { , <element table definition> }... ] )
//!       [ EDGE TABLES ( <element table definition> [ { , <element table definition> }... ] ) ]
//!       [ OPTIONS ( { ENFORCED | TRUSTED } MODE ) ] [ WITH INVOKER RIGHTS ]
//! <element table definition> ::= <table name> [ AS <element table alias> ]
//!     [ KEY ( <column name list> ) ]
//!     [ UNDIRECTED ] [ SOURCE <endpoint> DESTINATION <endpoint> ]   -- of an edge
//!     [ <label and properties>... | <element table properties> ]
//! <endpoint> ::= KEY ( <column name list> ) REFERENCES <element table alias> [ ( <column name list> ) ]
//!     | <element table alias>
//! <label and properties> ::= { LABEL <label name> | DEFAULT LABEL } [ <element table properties> ]
//! <element table properties> ::= NO PROPERTIES
//!     | PROPERTIES ARE ALL COLUMNS [ EXCEPT ( <column name list> ) ]
//!     | PROPERTIES ( <value expression> [ AS <property name> ] [ { , ... }... ] )
//! <alter property graph statement> ::= ALTER PROPERTY GRAPH <property graph name>
//!     { ADD { VERTEX | EDGE } TABLES ( <element table definition> [ { , ... }... ] )
//!     | DROP { VERTEX | EDGE } TABLES ( <element table alias> [ { , ... }... ] ) [ <drop behavior> ]
//!     | ALTER { VERTEX | EDGE } TABLE <element table alias> <alter element table action>
//!     | COMPILE }
//! <alter element table action> ::= ADD <label and properties>
//!     | DROP LABEL <label name> [ <drop behavior> ]
//!     | ALTER LABEL <label name> ADD PROPERTIES ( <value expression> [ AS <property name> ] [ { , ... }... ] )
//!     | ALTER LABEL <label name> DROP PROPERTIES ( <property name> [ { , ... }... ] ) [ <drop behavior> ]
//! <drop property graph statement> ::= DROP PROPERTY GRAPH [ IF EXISTS ] <property graph name> [ <drop behavior> ]
//! ```
//!
//! Each word but the reserved ones is a `<non-reserved word>`, so a table
//! or label may still be named by it.

use super::Parser;
use crate::ast::{
    AlterGraph, AlterGraphAction, DerivedProperty, DropBehavior, DropObject, ElementAction, ElementDefinition, EndpointDefinition,
    GraphDefinition, GraphModeOption, LabelDefinition, PropertiesDefinition, SchemaStatement, SqlStatement,
};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};

/// The `<non-reserved word>` PROPERTY.
pub const PROPERTY: &str = "PROPERTY";
/// The `<non-reserved word>` GRAPH.
pub const GRAPH: &str = "GRAPH";
/// The `<non-reserved word>` VERTEX.
const VERTEX: &str = "VERTEX";
/// The `<non-reserved word>` EDGE.
const EDGE: &str = "EDGE";
/// The `<non-reserved word>` TABLES.
const TABLES: &str = "TABLES";
/// The `<non-reserved word>` LABEL.
const LABEL: &str = "LABEL";
/// The `<non-reserved word>` PROPERTIES.
const PROPERTIES: &str = "PROPERTIES";
/// The `<non-reserved word>` COLUMNS.
const COLUMNS: &str = "COLUMNS";
/// The `<non-reserved word>` SOURCE.
const SOURCE: &str = "SOURCE";
/// The `<non-reserved word>` DESTINATION.
const DESTINATION: &str = "DESTINATION";
/// The `<non-reserved word>` UNDIRECTED.
const UNDIRECTED: &str = "UNDIRECTED";
/// The `<non-reserved word>` OPTIONS.
const OPTIONS: &str = "OPTIONS";
/// The `<non-reserved word>` ENFORCED.
const ENFORCED: &str = "ENFORCED";
/// The `<non-reserved word>` TRUSTED.
const TRUSTED: &str = "TRUSTED";
/// The `<non-reserved word>` MODE.
const MODE: &str = "MODE";
/// The `<non-reserved word>` IF.
const IF: &str = "IF";
/// The `<non-reserved word>` REPLACE.
const REPLACE: &str = "REPLACE";
/// The `<non-reserved word>` INVOKER.
const INVOKER: &str = "INVOKER";
/// The `<non-reserved word>` RIGHTS.
const RIGHTS: &str = "RIGHTS";
/// The `<non-reserved word>` COMPILE.
const COMPILE: &str = "COMPILE";

impl Parser {
    /// Whether PROPERTY GRAPH is `n` tokens ahead.
    pub(super) fn graph_at(&self, n: usize) -> bool {
        let word = |k: usize, w: &str| matches!(self.peek_at(k), Tok::Identifier(s) if s == w);
        word(n, PROPERTY) && word(n.saturating_add(1), GRAPH)
    }

    /// Whether a statement of property graphs begins here: CREATE \[OR
    /// REPLACE\], ALTER or DROP, then PROPERTY GRAPH.
    pub(super) fn next_is_graph(&self) -> bool {
        let replace = self.next_is_kw(kw::OR) && matches!(self.peek_at(2), Tok::Identifier(s) if s == REPLACE) && self.graph_at(3);
        (self.is_kw(kw::CREATE) || self.is_kw(kw::ALTER) || self.is_kw(kw::DROP)) && (self.graph_at(1) || replace)
    }

    /// A `<create property graph statement>` or an `<alter property graph
    /// statement>`, from CREATE or ALTER.
    ///
    /// # Errors
    ///
    /// The text is not one.
    pub(super) fn graph_statement(&mut self) -> Result<SchemaStatement> {
        if self.accept_kw(kw::ALTER) {
            self.advance();
            self.advance();
            let name = self.table_name()?;
            return Ok(SchemaStatement::AlterGraph(AlterGraph { name, action: self.alter_graph_action()? }));
        }
        Ok(SchemaStatement::Graph(Box::new(self.graph_definition()?)))
    }

    /// `<create property graph statement>`, from CREATE.
    ///
    /// # Errors
    ///
    /// The text is not one.
    pub(crate) fn graph_definition(&mut self) -> Result<GraphDefinition> {
        self.expect_kw(kw::CREATE)?;
        let replace = self.accept_kw(kw::OR);
        if replace {
            self.expect_word(REPLACE)?;
        }
        self.expect_word(PROPERTY)?;
        self.expect_word(GRAPH)?;
        let if_not_exists = self.is_word(IF) && self.next_is_kw(kw::NOT);
        if if_not_exists {
            self.advance();
            self.advance();
            self.expect_kw(kw::EXISTS)?;
        }
        if replace && if_not_exists {
            return Err(self.syntax("expected OR REPLACE or IF NOT EXISTS, not both"));
        }
        let name = self.table_name()?;
        self.expect_word(VERTEX)?;
        self.expect_word(TABLES)?;
        let vertices = self.element_list(false)?;
        let edges = if self.accept_word(EDGE) {
            self.expect_word(TABLES)?;
            self.element_list(true)?
        } else {
            Vec::new()
        };
        let mode = self.graph_mode()?;
        let invoker = self.accept_kw(kw::WITH);
        if invoker {
            self.expect_word(INVOKER)?;
            self.expect_word(RIGHTS)?;
        }
        Ok(GraphDefinition { name, replace, if_not_exists, vertices, edges, mode, invoker })
    }

    /// OPTIONS ( { ENFORCED | TRUSTED } MODE ), if written.
    fn graph_mode(&mut self) -> Result<Option<GraphModeOption>> {
        if !self.accept_word(OPTIONS) {
            return Ok(None);
        }
        self.expect_delim(delim::LEFT_PAREN)?;
        let mode = if self.accept_word(ENFORCED) {
            GraphModeOption::Enforced
        } else if self.accept_word(TRUSTED) {
            GraphModeOption::Trusted
        } else {
            return Err(self.syntax("expected ENFORCED or TRUSTED"));
        };
        self.expect_word(MODE)?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(Some(mode))
    }

    /// ( `<element table definition>` \[{, ...}...\] ), of edges if `edges`.
    fn element_list(&mut self, edges: bool) -> Result<Vec<ElementDefinition>> {
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut all = vec![self.element_definition(edges)?];
        while self.accept_delim(delim::COMMA) {
            all.push(self.element_definition(edges)?);
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(all)
    }

    /// `<element table definition>`, of an edge if `edge`.
    fn element_definition(&mut self, edge: bool) -> Result<ElementDefinition> {
        let table = self.table_name()?;
        let alias = if self.accept_kw(kw::AS) { Some(self.label_named()?) } else { None };
        let key = if self.accept_kw(kw::KEY) { Some(self.column_list()?) } else { None };
        let (mut undirected, mut source, mut destination) = (false, None, None);
        if edge {
            undirected = self.accept_word(UNDIRECTED);
            self.expect_word(SOURCE)?;
            source = Some(self.endpoint()?);
            self.expect_word(DESTINATION)?;
            destination = Some(self.endpoint()?);
        }
        let labels = self.labels_and_properties()?;
        Ok(ElementDefinition { table, alias, key, undirected, source, destination, labels })
    }

    /// `<endpoint>`, after SOURCE or DESTINATION.
    fn endpoint(&mut self) -> Result<EndpointDefinition> {
        let key = if self.accept_kw(kw::KEY) {
            let key = self.column_list()?;
            self.expect_kw(kw::REFERENCES)?;
            Some(key)
        } else {
            None
        };
        let at = self.here();
        let vertex = (self.identifier()?, at);
        let references = if key.is_some() && self.is_delim(delim::LEFT_PAREN) { Some(self.column_list()?) } else { None };
        Ok(EndpointDefinition { key, vertex, references })
    }

    /// The `<label and properties>` of an element table: those written, or
    /// one of the default label of `<element table properties>` alone; none
    /// if neither is.
    fn labels_and_properties(&mut self) -> Result<Vec<LabelDefinition>> {
        let mut labels = Vec::new();
        while let Some(label) = self.label_and_properties()? {
            labels.push(label);
        }
        if labels.is_empty() {
            let at = self.here();
            if let Some(properties) = self.element_properties()? {
                labels.push(LabelDefinition { name: None, at, properties: Some(properties) });
            }
        }
        Ok(labels)
    }

    /// `<label and properties>`, if one is next.
    fn label_and_properties(&mut self) -> Result<Option<LabelDefinition>> {
        let at = self.here();
        let name = if self.accept_word(LABEL) {
            Some(self.identifier()?)
        } else if self.is_kw(kw::DEFAULT) && matches!(self.peek_at(1), Tok::Identifier(s) if s == LABEL) {
            self.advance();
            self.advance();
            None
        } else {
            return Ok(None);
        };
        Ok(Some(LabelDefinition { name, at, properties: self.element_properties()? }))
    }

    /// `<element table properties>`, if written.
    fn element_properties(&mut self) -> Result<Option<PropertiesDefinition>> {
        if self.is_kw(kw::NO) && matches!(self.peek_at(1), Tok::Identifier(s) if s == PROPERTIES) {
            self.advance();
            self.advance();
            return Ok(Some(PropertiesDefinition::None));
        }
        if !self.accept_word(PROPERTIES) {
            return Ok(None);
        }
        if self.accept_kw(kw::ARE) {
            self.expect_kw(kw::ALL)?;
            self.expect_word(COLUMNS)?;
            let except = if self.accept_kw(kw::EXCEPT) { self.column_list()? } else { Vec::new() };
            return Ok(Some(PropertiesDefinition::AllColumns(except)));
        }
        Ok(Some(PropertiesDefinition::Listed(self.derived_properties()?)))
    }

    /// ( `<derived property>` \[{, `<derived property>`}...\] )
    fn derived_properties(&mut self) -> Result<Vec<DerivedProperty>> {
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut all = vec![self.derived_property()?];
        while self.accept_delim(delim::COMMA) {
            all.push(self.derived_property()?);
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(all)
    }

    /// `<derived property>`: a column alone is named after it.
    fn derived_property(&mut self) -> Result<DerivedProperty> {
        let at = self.here();
        let ends = matches!(self.peek_at(1), Tok::Delimiter(d) if *d == delim::COMMA || *d == delim::RIGHT_PAREN);
        let alone = if let (Tok::Identifier(name), true) = (self.peek(), ends) { Some(name.clone()) } else { None };
        self.value_expression()?;
        // A column alone is kept as its name written as an identifier.
        let text = alone.as_deref().map_or_else(|| self.text_from(at), crate::catalog::identifier);
        let name = if self.accept_kw(kw::AS) { Some(self.identifier()?) } else { alone };
        Ok(DerivedProperty { text, name, at })
    }

    /// `<alter property graph action>`, after the graph's name.
    fn alter_graph_action(&mut self) -> Result<AlterGraphAction> {
        if self.accept_word(COMPILE) {
            return Ok(AlterGraphAction::Compile);
        }
        if self.accept_kw(kw::ADD) {
            let edges = self.vertex_or_edge()?;
            self.expect_word(TABLES)?;
            return Ok(AlterGraphAction::Add { edges, tables: self.element_list(edges)? });
        }
        if self.accept_kw(kw::DROP) {
            let edges = self.vertex_or_edge()?;
            self.expect_word(TABLES)?;
            let aliases = self.alias_list()?;
            return Ok(AlterGraphAction::Drop { edges, aliases, behavior: self.optional_behavior()? });
        }
        self.expect_kw(kw::ALTER)?;
        let edge = self.vertex_or_edge()?;
        self.expect_kw(kw::TABLE)?;
        let at = self.here();
        let alias = (self.identifier()?, at);
        Ok(AlterGraphAction::Element { edge, alias, action: self.element_action()? })
    }

    /// VERTEX or EDGE: whether EDGE.
    fn vertex_or_edge(&mut self) -> Result<bool> {
        if self.accept_word(EDGE) {
            return Ok(true);
        }
        self.expect_word(VERTEX)?;
        Ok(false)
    }

    /// ( `<element table alias>` \[{, ...}...\] ), each with where it begins.
    fn alias_list(&mut self) -> Result<Vec<(String, usize)>> {
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut all = Vec::new();
        loop {
            let at = self.here();
            all.push((self.identifier()?, at));
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(all)
    }

    /// A `<drop behavior>` if written, else RESTRICT.
    pub(super) fn optional_behavior(&mut self) -> Result<DropBehavior> {
        if self.is_kw(kw::CASCADE) || self.is_kw(kw::RESTRICT) { self.drop_behavior() } else { Ok(DropBehavior::Restrict) }
    }

    /// `<alter element table action>`.
    fn element_action(&mut self) -> Result<ElementAction> {
        if self.accept_kw(kw::ADD) {
            return match self.label_and_properties()? {
                Some(label) => Ok(ElementAction::AddLabel(label)),
                None => Err(self.syntax("expected LABEL or DEFAULT LABEL")),
            };
        }
        if self.accept_kw(kw::DROP) {
            self.expect_word(LABEL)?;
            let label = self.label_named()?;
            return Ok(ElementAction::DropLabel { label, behavior: self.optional_behavior()? });
        }
        self.expect_kw(kw::ALTER)?;
        self.expect_word(LABEL)?;
        let label = self.label_named()?;
        if self.accept_kw(kw::ADD) {
            self.expect_word(PROPERTIES)?;
            return Ok(ElementAction::AddProperties { label, properties: self.derived_properties()? });
        }
        self.expect_kw(kw::DROP)?;
        self.expect_word(PROPERTIES)?;
        let names = self.column_list()?;
        Ok(ElementAction::DropProperties { label, names, behavior: self.optional_behavior()? })
    }

    /// An identifier, a `<label name>` or an alias, and where it begins.
    fn label_named(&mut self) -> Result<(String, usize)> {
        let at = self.here();
        Ok((self.identifier()?, at))
    }

    /// `<drop property graph statement>`, after DROP, if PROPERTY GRAPH is
    /// next.
    ///
    /// # Errors
    ///
    /// The text after PROPERTY GRAPH is not the rest of one.
    pub(super) fn drop_graph(&mut self) -> Result<Option<SqlStatement>> {
        if !self.graph_at(0) {
            return Ok(None);
        }
        self.advance();
        self.advance();
        let if_exists = self.is_word(IF) && self.next_is_kw(kw::EXISTS);
        if if_exists {
            self.advance();
            self.advance();
        }
        let name = self.table_name()?;
        Ok(Some(SqlStatement::Drop { object: DropObject::Graph(name, if_exists), behavior: self.optional_behavior()? }))
    }
}
