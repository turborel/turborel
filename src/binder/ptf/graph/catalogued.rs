//! \[EXTENSION\] A property graph of the catalog as the edges of a
//! function of GRAPH_ALGO, the graph standard's §18 (RULES.md
//! D-GRAPH-ALGO): `PAGERANK(g)`, a graph of CREATE PROPERTY GRAPH or a
//! schema's SYS_GRAPH of its node and edge tables (§4.2).
//!
//! The graph is read as two queries of its element tables, bound as table
//! arguments are, so with the statement's privileges on those tables
//! [GS §18 R18.2]. The edges are each edge table's source and destination
//! columns, and then the columns every edge table has alike, of one name
//! and type, so COST may name one. An undirected edge table gives each
//! edge both ways. An edge table a graph names more than once, as
//! SYS_GRAPH names one for each pair of node tables it may join, is read
//! once. The vertices are each vertex table's key, unless VERTICES is
//! given.
//!
//! A vertex is its key, so a key is of one column, and an endpoint
//! references its vertex table's key. Of a graph of more than one vertex
//! table, two tables' keys could be equal, so only a SYS_GRAPH, whose
//! keys are identities of their own tables (R5.1), may have more.

use std::collections::BTreeSet;

use super::super::super::from::Bounds;
use super::super::tables::{BoundTable, table_argument};
use super::{WHOLE, refused};
use crate::ast::Subquery;
use crate::ast::{TableArgument, TableArgumentProper, ValueExpression};
use crate::catalog::{ColumnDesc, ElementKind, ElementTableDesc, GraphDesc, QualifiedName, SYS_GRAPH};
use crate::error::{Code, Result, SqlError};
use crate::graph::algo::Algorithm;
use crate::parser::Parser;

/// The property graph the value argument `v` names, if it is a name of
/// one: `g` of the default schema, or `s.g`.
pub(super) fn graph(b: Bounds<'_>, v: &ValueExpression) -> Option<GraphDesc> {
    let ValueExpression::Column(spec) = v else { return None };
    if spec.catalog.is_some() || spec.qualifier.as_ref().is_some_and(|q| q.auth.is_some()) {
        return None;
    }
    let auth = spec.qualifier.as_ref().map_or(b.cx.schema, |q| q.ident.as_str()).to_owned();
    b.cx.db.catalog.graph(&QualifiedName { auth, ident: spec.column.clone() }).cloned()
}

/// The edges of graph `g`, and its vertices, bound as table arguments of
/// `algorithm`, nested `depth` deep.
///
/// # Errors
///
/// SQLCODE -103 if `g` is not valid; -900 if a key or an endpoint is of
/// more than one column, an endpoint references other columns than its
/// vertex table's key, or `g` is not a SYS_GRAPH and has more than one
/// vertex table; any of reading its tables.
pub(super) fn bound(b: Bounds<'_>, algorithm: Algorithm, g: &GraphDesc, depth: usize) -> Result<(BoundTable, BoundTable)> {
    if !g.valid {
        return Err(refused(algorithm, &format!("graph {} is not valid until ALTER PROPERTY GRAPH ... COMPILE finds it holds", g.name)));
    }
    let vertices: Vec<&ElementTableDesc> = g.elements.iter().filter(|e| e.kind == ElementKind::Vertex).collect();
    if vertices.len() > 1 && g.name.ident != SYS_GRAPH {
        return Err(unsupported(algorithm, g, "has more than one vertex table, whose keys could be equal"));
    }
    let keys = vertices.iter().map(|v| single(algorithm, g, &v.key).map(|k| (v, k))).collect::<Result<Vec<_>>>()?;
    let edges = edge_selects(b, algorithm, g)?;
    let edges = if edges.is_empty() {
        // Of no edge table: the vertices' keys twice, of no row.
        let (v, k) = keys.first().ok_or_else(|| refused(algorithm, &format!("graph {} has no vertex table", g.name)))?;
        vec![format!("SELECT {k}, {k} FROM {} WHERE 0 = 1", table(&v.table))]
    } else {
        edges
    };
    let selects: Vec<String> = keys.iter().map(|(v, k)| format!("SELECT {k} FROM {}", table(&v.table))).collect();
    Ok((argument(b, &edges, depth)?, argument(b, &selects, depth)?))
}

/// The SELECTs of the edges of `g`, each edge table's once, an undirected
/// one's both ways, with the columns all its edge tables have alike.
fn edge_selects(b: Bounds<'_>, algorithm: Algorithm, g: &GraphDesc) -> Result<Vec<String>> {
    let mut seen = BTreeSet::new();
    let mut ends = Vec::new();
    for e in g.elements.iter().filter(|e| e.kind == ElementKind::Edge) {
        let Some(points) = &e.endpoints else { continue };
        let column = |p: &crate::catalog::EndpointDesc| {
            let vertex = g.elements.iter().find(|v| v.alias == p.vertex && v.kind == ElementKind::Vertex);
            if vertex.is_none_or(|v| v.key != p.references) {
                return Err(unsupported(algorithm, g, "has an endpoint that references other columns than its vertex table's key"));
            }
            single(algorithm, g, &p.columns)
        };
        let (source, destination) = (column(&points.source)?, column(&points.destination)?);
        if seen.insert((e.table.clone(), source.clone(), destination.clone(), points.undirected)) {
            ends.push((e, source, destination, points.undirected));
        }
    }
    let alike = alike(b, &ends.iter().map(|(e, ..)| *e).collect::<Vec<_>>());
    let extra: String = alike.iter().flat_map(|c| [", ".to_owned(), quoted(&c.name)]).collect();
    let mut selects = Vec::new();
    for (e, source, destination, undirected) in &ends {
        selects.push(format!("SELECT {source}, {destination}{extra} FROM {}", table(&e.table)));
        if *undirected {
            selects.push(format!("SELECT {destination}, {source}{extra} FROM {}", table(&e.table)));
        }
    }
    Ok(selects)
}

/// The columns each of the edge tables `edges` has, of one name and type,
/// but their endpoints' and hidden ones, in the first's order.
fn alike(b: Bounds<'_>, edges: &[&ElementTableDesc]) -> Vec<ColumnDesc> {
    let columns = |e: &ElementTableDesc| -> Vec<ColumnDesc> {
        let catalog = &b.cx.db.catalog;
        let all = catalog.table(&e.table).map(|t| t.columns.clone()).or_else(|| catalog.view(&e.table).map(|v| v.columns.clone()));
        let named = |c: &ColumnDesc| {
            e.endpoints.as_ref().is_some_and(|p| p.source.columns.contains(&c.name) || p.destination.columns.contains(&c.name))
        };
        all.unwrap_or_default().into_iter().filter(|c| !c.hidden() && !named(c)).collect()
    };
    let Some((first, rest)) = edges.split_first() else { return Vec::new() };
    let others: Vec<Vec<ColumnDesc>> = rest.iter().map(|e| columns(e)).collect();
    let shared = |c: &ColumnDesc| others.iter().all(|o| o.iter().any(|d| d.name == c.name && d.data_type == c.data_type));
    columns(first).into_iter().filter(|c| shared(c)).collect()
}

/// The one column of `columns`, a key's or an endpoint's, quoted.
fn single(algorithm: Algorithm, g: &GraphDesc, columns: &[String]) -> Result<String> {
    match columns {
        [one] => Ok(quoted(one)),
        _ => Err(unsupported(algorithm, g, "has a key or an endpoint of more than one column")),
    }
}

/// The refusal of graph `g`, which `why`, as the edges of `algorithm`.
fn unsupported(algorithm: Algorithm, g: &GraphDesc, why: &str) -> SqlError {
    let msg = format!("[EXTENSION] GRAPH_ALGO.{}: graph {} {why}: valid but not implemented", algorithm.name(), g.name);
    SqlError::new(Code::Unsupported, msg)
}

/// `name` as a `<delimited identifier>`.
fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Table `t`'s name, quoted.
fn table(t: &QualifiedName) -> String {
    format!("{}.{}", quoted(&t.auth), quoted(&t.ident))
}

/// The UNION ALL of `selects` bound as a table argument, nested `depth`
/// deep.
fn argument(b: Bounds<'_>, selects: &[String], depth: usize) -> Result<BoundTable> {
    let spec = Parser::new(&selects.join(" UNION ALL "))?.cursor_specification()?;
    let proper = TableArgumentProper::Query(Box::new(Subquery { query: spec.query, at: 0 }));
    let ta = TableArgument { proper, correlation: None, columns: None, partition: None, prune: None, order: Vec::new(), at: 0 };
    table_argument(b, &ta, WHOLE, depth)
}
