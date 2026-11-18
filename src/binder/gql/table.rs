//! \[EXTENSION\] GQL_TABLE ( `<graph name>` , `<GQL text>` ): a GQL
//! query's table as a `<table primary>` of SQL (graph standard §9), its
//! lowering (`binder/gql.rs`) bound as a derived table is.

use super::{Graph, gql_source, lower};
use crate::ast::{DerivedTable, GqlRequest, GqlTable, Subquery, TableName};
use crate::binder::{Context, qualify};
use crate::catalog::{Catalog, Who};
use crate::error::{Code, Result, SqlError};
use crate::parser::Parser;

/// \[EXTENSION\] The graph `name`, of the schema `schema` if unqualified,
/// that `who` may query: its descriptor, and whether it is an open GQL
/// graph.
///
/// # Errors
///
/// SQLCODE -103 (GRF-0001) if there is no property graph of the name; as
/// `GraphDesc::queryable`.
pub(crate) fn graph_of(catalog: &dyn Catalog, who: Who<'_>, schema: &str, name: &TableName) -> Result<Graph> {
    let qn = qualify(name, schema);
    let Some(desc) = catalog.graph(&qn) else {
        return Err(SqlError::at(Code::SyntaxRule, name.at, format!("[EXTENSION] GRF-0001 there is no property graph {qn}")));
    };
    desc.queryable(catalog, who)?;
    let gql = catalog.gql().gql_graph(&qn);
    let open = gql.is_some_and(|g| g.graph_type.is_none());
    let closed = gql.is_some_and(|g| g.graph_type.is_some());
    Ok(Graph { desc: desc.clone(), open, closed })
}

/// \[EXTENSION\] The GQL query `text`, of the graph `graph` unless a
/// statement USEs another, lowered to an SQL query's text and the names
/// of its columns.
///
/// # Errors
///
/// SQLCODE -102 if `text` is no GQL procedure; -900 if it is a session or
/// transaction statement or modifies data; as `lower`.
pub(crate) fn query_text(text: &str, graphs: super::Graphs<'_>) -> Result<super::Lowered> {
    let source = gql_source(text)?;
    let GqlRequest::Procedure(p) = Parser::new(&source)?.gql_request()? else {
        return Err(SqlError::new(Code::Unsupported, "[EXTENSION] GQL_TABLE reads a GQL query, not a session or transaction statement"));
    };
    lower(&source, &p, graphs)
}

/// \[EXTENSION\] `g` as the derived table its lowering is.
///
/// # Errors
///
/// As `query_text`, and as the lowered query's reading.
pub(crate) fn derived_of(cx: Context<'_>, g: &GqlTable) -> Result<DerivedTable> {
    let catalog = cx.db.catalog.as_ref();
    let who = cx.who();
    let graphs = |name: Option<&TableName>| graph_of(catalog, who, cx.schema, name.unwrap_or(&g.graph));
    let lowered = query_text(&g.text, &graphs)?;
    let query = Parser::gql_lowered(&lowered.sql)?;
    let correlation = g.correlation.clone().unwrap_or_else(|| g.graph.ident.clone());
    Ok(DerivedTable { query: Box::new(Subquery { query, at: g.at }), correlation, columns: g.columns.clone() })
}
