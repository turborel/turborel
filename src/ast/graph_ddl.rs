//! \[EXTENSION\] Abstract syntax of the statements that define property
//! graphs over tables (RULES.md D106): CREATE, ALTER and DROP
//! PROPERTY GRAPH, Mode A of the property graph implementation standard
//! (§4.1), the form of SQL/PGQ, ISO/IEC 9075-16:2023.

use super::{DropBehavior, TableName};

/// `<create property graph statement>` ::= CREATE \[OR REPLACE\] PROPERTY
/// GRAPH \[IF NOT EXISTS\] `<property graph name>` `<vertex tables clause>`
/// \[`<edge tables clause>`\] \[OPTIONS ( `<graph mode>` )\] \[WITH INVOKER
/// RIGHTS\]
#[derive(Debug, Clone)]
pub struct GraphDefinition {
    /// The `<property graph name>`.
    pub name: TableName,
    /// OR REPLACE was written: a graph of the name is replaced.
    pub replace: bool,
    /// IF NOT EXISTS was written: a graph of the name is kept, and nothing
    /// is defined.
    pub if_not_exists: bool,
    /// The `<vertex table definition>`s, in order.
    pub vertices: Vec<ElementDefinition>,
    /// The `<edge table definition>`s, in order.
    pub edges: Vec<ElementDefinition>,
    /// The `<graph mode>` of OPTIONS, if written: ENFORCED if not.
    pub mode: Option<GraphModeOption>,
    /// WITH INVOKER RIGHTS was written: a query reads its element tables
    /// with the privileges of its user too.
    pub invoker: bool,
}

/// `<graph mode>` ::= ENFORCED MODE | TRUSTED MODE
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphModeOption {
    /// Keys and endpoint references are declared constraints.
    Enforced,
    /// Keys and endpoint references are taken as written.
    Trusted,
}

/// `<vertex table definition>` ::= `<element table name>` \[AS `<element
/// table alias>`\] \[`<element table key clause>`\] \[`<element table label
/// and properties>`\]; `<edge table definition>` adds \[UNDIRECTED\]
/// `<source vertex table>` `<destination vertex table>` before its labels.
#[derive(Debug, Clone)]
pub struct ElementDefinition {
    /// The `<element table name>`: a base table or a view.
    pub table: TableName,
    /// The `<element table alias>`, if written, and where it begins.
    pub alias: Option<(String, usize)>,
    /// `<element table key clause>` ::= KEY ( `<column name list>` ), if
    /// written.
    pub key: Option<Vec<String>>,
    /// Of an edge, UNDIRECTED was written.
    pub undirected: bool,
    /// Of an edge, its `<source vertex table>`.
    pub source: Option<EndpointDefinition>,
    /// Of an edge, its `<destination vertex table>`.
    pub destination: Option<EndpointDefinition>,
    /// Its `<label and properties>`, in order: none if neither a label nor
    /// properties are written.
    pub labels: Vec<LabelDefinition>,
}

impl ElementDefinition {
    /// Its alias: the one written, else its table's identifier.
    pub fn alias_name(&self) -> &str {
        self.alias.as_ref().map_or(&self.table.ident, |(a, _)| a)
    }

    /// Where its alias begins: the alias's, else its table's.
    pub fn alias_at(&self) -> usize {
        self.alias.as_ref().map_or(self.table.at, |(_, at)| *at)
    }
}

/// `<source vertex table>` ::= SOURCE \[KEY ( `<column name list>` )
/// REFERENCES\] `<element table alias>` \[( `<column name list>` )\], and
/// `<destination vertex table>` alike after DESTINATION.
#[derive(Debug, Clone)]
pub struct EndpointDefinition {
    /// The columns of the edge table of KEY, if written.
    pub key: Option<Vec<String>>,
    /// The alias of the vertex table referenced, and where it begins.
    pub vertex: (String, usize),
    /// The columns of the vertex table referenced, if written.
    pub references: Option<Vec<String>>,
}

/// `<label and properties>` ::= `<element table label clause>`
/// \[`<element table properties>`\], or `<element table properties>`
/// alone, of the default label.
#[derive(Debug, Clone)]
pub struct LabelDefinition {
    /// LABEL `<label name>`, or none for DEFAULT LABEL.
    pub name: Option<String>,
    /// Where it begins.
    pub at: usize,
    /// Its `<element table properties>`, if written: ARE ALL COLUMNS if
    /// not.
    pub properties: Option<PropertiesDefinition>,
}

/// `<element table properties>` ::= NO PROPERTIES | PROPERTIES ARE ALL
/// COLUMNS \[EXCEPT ( `<column name list>` )\] | PROPERTIES (
/// `<derived property list>` )
#[derive(Debug, Clone)]
pub enum PropertiesDefinition {
    /// NO PROPERTIES.
    None,
    /// ARE ALL COLUMNS, but those EXCEPT names.
    AllColumns(Vec<String>),
    /// The `<derived property>`s, in order.
    Listed(Vec<DerivedProperty>),
}

/// `<derived property>` ::= `<value expression>` \[AS `<property name>`\]
#[derive(Debug, Clone)]
pub struct DerivedProperty {
    /// The `<value expression>` as written.
    pub text: String,
    /// The `<property name>`: the one written, else the column's of a
    /// value expression that is a column reference alone.
    pub name: Option<String>,
    /// Where it begins.
    pub at: usize,
}

/// `<alter property graph statement>` ::= ALTER PROPERTY GRAPH `<property
/// graph name>` `<alter property graph action>`
#[derive(Debug, Clone)]
pub struct AlterGraph {
    /// The `<property graph name>`.
    pub name: TableName,
    /// What it does.
    pub action: AlterGraphAction,
}

/// `<alter property graph action>`
#[derive(Debug, Clone)]
pub enum AlterGraphAction {
    /// ADD VERTEX TABLES ( ... ) or ADD EDGE TABLES ( ... ): of edges if
    /// `edges`.
    Add {
        /// Whether they are edge tables.
        edges: bool,
        /// The element table definitions.
        tables: Vec<ElementDefinition>,
    },
    /// DROP VERTEX TABLES ( `<alias list>` ) or DROP EDGE TABLES ( ... )
    /// \[`<drop behavior>`\].
    Drop {
        /// Whether they are edge tables.
        edges: bool,
        /// Each alias, and where it begins.
        aliases: Vec<(String, usize)>,
        /// The `<drop behavior>`: RESTRICT if none is written.
        behavior: DropBehavior,
    },
    /// ALTER {VERTEX | EDGE} TABLE `<alias>` `<alter element table action>`.
    Element {
        /// Whether it is an edge table.
        edge: bool,
        /// Its alias, and where it begins.
        alias: (String, usize),
        /// What it does.
        action: ElementAction,
    },
    /// COMPILE: the graph checked again, and valid if it holds.
    Compile,
}

/// `<alter element table action>`
#[derive(Debug, Clone)]
pub enum ElementAction {
    /// ADD LABEL `<label name>` \[`<element table properties>`\].
    AddLabel(LabelDefinition),
    /// DROP LABEL `<label name>` \[`<drop behavior>`\].
    DropLabel {
        /// The label, and where it begins.
        label: (String, usize),
        /// The `<drop behavior>`: RESTRICT if none is written.
        behavior: DropBehavior,
    },
    /// ALTER LABEL `<label name>` ADD PROPERTIES ( `<derived property
    /// list>` ).
    AddProperties {
        /// The label, and where it begins.
        label: (String, usize),
        /// The properties added.
        properties: Vec<DerivedProperty>,
    },
    /// ALTER LABEL `<label name>` DROP PROPERTIES ( `<property name
    /// list>` ) \[`<drop behavior>`\].
    DropProperties {
        /// The label, and where it begins.
        label: (String, usize),
        /// The properties dropped.
        names: Vec<String>,
        /// The `<drop behavior>`: RESTRICT if none is written.
        behavior: DropBehavior,
    },
}

/// \[EXTENSION\] [GS §13.2] CREATE ADJACENCY INDEX \[`<index name>`\] ON
/// GRAPH `<graph name>` EDGE TABLE `<alias>` \[DIRECTION {FORWARD |
/// REVERSE | BOTH}\] \[INCLUDE ( `<column name>` \[, ...\] )\]: an index
/// of the adjacency method of the edge table's ends.
#[derive(Debug, Clone)]
pub struct AdjacencyIndex {
    /// The `<index name>`, if written, and where it begins.
    pub name: Option<(String, usize)>,
    /// The graph.
    pub graph: TableName,
    /// The edge table's alias in the graph, and where it begins.
    pub edge: (String, usize),
    /// FORWARD, REVERSE or BOTH, if written.
    pub direction: Option<String>,
    /// The columns INCLUDE names, and where each begins.
    pub include: Vec<(String, usize)>,
}

/// \[EXTENSION\] CREATE LABEL INDEX \[`<index name>`\] ON GRAPH `<graph
/// name>` \[{VERTEX | EDGE} TABLE `<alias>`\]: an index of the labels
/// method of an element table's key, or of each base element table's.
#[derive(Debug, Clone)]
pub struct LabelIndex {
    /// The `<index name>`, if written, and where it begins.
    pub name: Option<(String, usize)>,
    /// The graph.
    pub graph: TableName,
    /// The element table's alias in the graph, whether it is an edge
    /// table, and where it begins; none for every element table.
    pub table: Option<(String, bool, usize)>,
}

/// \[EXTENSION\] CREATE REACHABILITY INDEX \[`<index name>`\] ON GRAPH
/// `<graph name>` EDGE TABLE `<alias>`: an index of the reachability
/// method of the edge table's ends.
#[derive(Debug, Clone)]
pub struct ReachabilityIndex {
    /// The `<index name>`, if written, and where it begins.
    pub name: Option<(String, usize)>,
    /// The graph.
    pub graph: TableName,
    /// The edge table's alias in the graph, and where it begins.
    pub edge: (String, usize),
}
