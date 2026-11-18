//! \[EXTENSION\] [GS §10] The compatibility syntax's own parts of the
//! tree: MATCH in a WHERE clause over the tables of the FROM clause
//! [GS §10.1], its FOR PATH tables and WITHIN GROUP (GRAPH PATH)
//! aggregates, and a GRAPH_TABLE's hints [GS §10.2, R16.8]. Each holds
//! GPML already: the front ends compile to the one GPML tree [GS §14],
//! and the binder only names each variable's graph and labels.

use super::PathPattern;
use crate::ast::{TableName, ValueExpression};

/// \[EXTENSION\] [GS §10.1] MATCH ( `<graph search pattern>` ) in a
/// WHERE clause: its path patterns in GPML, each variable named as the
/// FROM clause's table it stands for.
#[derive(Debug, Clone)]
pub struct MatchClause {
    /// The path patterns, in order: one of each simple match pattern, and
    /// one of each arbitrary length pattern of SHORTEST_PATH.
    pub paths: Vec<PathPattern>,
    /// Each LAST_NODE ( x ) = LAST_NODE ( y ), as the two end node
    /// variables it compares with SAME.
    pub ends: Vec<(String, String)>,
    /// The FOR PATH tables of the query, given to the MATCH that is a
    /// conjunct of the WHERE clause.
    pub path_tables: Vec<PathTable>,
    /// The WITHIN GROUP (GRAPH PATH) aggregates of the query, given as
    /// `path_tables` are.
    pub measures: Vec<GraphMeasure>,
    /// Where MATCH is written.
    pub at: usize,
}

/// \[EXTENSION\] [GS §10.1] A table of the FROM clause written FOR PATH:
/// a group variable of the pattern.
#[derive(Debug, Clone)]
pub struct PathTable {
    /// The table.
    pub table: TableName,
    /// The name it is exposed by: its correlation name, or its table's.
    pub name: String,
}

/// \[EXTENSION\] [GS §10.1] An aggregate WITHIN GROUP (GRAPH PATH): the
/// GPML value it is over the path's group variables, which the query
/// reads as the column `name` of the match.
#[derive(Debug, Clone)]
pub struct GraphMeasure {
    /// The column's name.
    pub name: String,
    /// Its value in GPML.
    pub value: ValueExpression,
    /// Where it is written.
    pub at: usize,
}
