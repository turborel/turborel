//! \[EXTENSION\] The definitions of Mode C of the property graph
//! implementation standard (§4.3): CREATE GRAPH TYPE, its node and edge
//! types, and CREATE GRAPH, typed or open.

use super::super::TableName;
use crate::value::DataType;

/// CREATE GRAPH TYPE \[IF NOT EXISTS\] `<name>` { `<element type>` \[,
/// ...\] }.
#[derive(Debug, Clone)]
pub struct GraphTypeDefinition {
    /// Its name.
    pub name: TableName,
    /// IF NOT EXISTS was written.
    pub if_not_exists: bool,
    /// Its node types, in order.
    pub nodes: Vec<NodeTypeDefinition>,
    /// Its edge types, in order.
    pub edges: Vec<EdgeTypeDefinition>,
}

/// NODE \[TYPE\] `<name>` \[LABEL\[S\] `<label>` \[& ...\]\] \[(\] {
/// `<property type>` \[, ...\] } \[)\].
#[derive(Debug, Clone, PartialEq)]
pub struct NodeTypeDefinition {
    /// Its name, its first label if none is written.
    pub name: String,
    /// Its labels, in order, one at least.
    pub labels: Vec<String>,
    /// Its property types, in order.
    pub properties: Vec<PropertyTypeDefinition>,
    /// Where it begins.
    pub at: usize,
}

/// \[DIRECTED | UNDIRECTED\] EDGE \[TYPE\] `<name>` \[labels\]
/// \[properties\] CONNECTING ( `<node type>` {-> | ~} `<node type>` ).
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeTypeDefinition {
    /// Its name, its first label if none is written.
    pub name: String,
    /// Its labels, in order, one at least.
    pub labels: Vec<String>,
    /// Its property types, in order.
    pub properties: Vec<PropertyTypeDefinition>,
    /// Whether it is directed.
    pub directed: bool,
    /// The node type its edges leave.
    pub source: String,
    /// The node type its edges reach.
    pub destination: String,
    /// Where it begins.
    pub at: usize,
}

/// `<property name>` `<value type>` \[NOT NULL\].
#[derive(Debug, Clone, PartialEq)]
pub struct PropertyTypeDefinition {
    /// Its name.
    pub name: String,
    /// Its type, as SQL holds it.
    pub data_type: DataType,
    /// NOT NULL was written.
    pub not_null: bool,
}

/// How a GQL graph is typed.
#[derive(Debug, Clone)]
pub enum GraphTyping {
    /// TYPED `<graph type name>`, or `::` it, or LIKE it: closed.
    Typed(TableName),
    /// ANY \[PROPERTY GRAPH\], or none written: open.
    Open,
}

/// CREATE \[PROPERTY\] GRAPH \[IF NOT EXISTS\] `<name>` \[TYPED `<type>` |
/// ANY\].
#[derive(Debug, Clone)]
pub struct GqlGraphDefinition {
    /// Its name.
    pub name: TableName,
    /// IF NOT EXISTS was written.
    pub if_not_exists: bool,
    /// How it is typed.
    pub typing: GraphTyping,
}
