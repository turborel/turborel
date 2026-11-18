//! \[EXTENSION\] A property graph of the catalog as the edges of a
//! function of GRAPH_ALGO, the graph standard's §18 (RULES.md
//! D-GRAPH-ALGO): `PAGERANK(g)`, a graph of CREATE PROPERTY GRAPH or a
//! schema's SYS_GRAPH of its node and edge tables (§4.2).
//!
//! The graph is read as a query of each of its element tables, bound as
//! a table argument is, so with the statement's privileges on those
//! tables [GS §18 R18.2]. A vertex table gives its key's columns. An edge
//! table gives its source's key and its destination's, read from its
//! vertex tables' rows that its endpoints reference, so an endpoint may
//! reference any of their columns and an edge of no vertex is none; then
//! the columns every edge table has alike, of one name and type, so COST
//! may name one. Each vertex is its identity, its element table and key,
//! made of these rows as the plan runs (`db/ptf/graph/shape.rs`).

use super::super::super::from::Bounds;
use super::super::tables::{BoundTable, table_argument};
use super::{WHOLE, refused};
use crate::ast::Subquery;
use crate::ast::{TableArgument, TableArgumentProper, ValueExpression};
use crate::catalog::{ColumnDesc, ElementKind, ElementTableDesc, EndpointDesc, GraphDesc, QualifiedName, SYS_GRAPH};
use crate::db::ptf::{EdgeShape, GraphShape, identity_type};
use crate::error::Result;
use crate::graph::algo::Algorithm;
use crate::parser::Parser;
use crate::value::DataType;

/// The correlation name of an edge table in its query.
const EDGE: &str = "GRAPH_ALGO_EDGE";
/// The correlation name of an edge's source's vertex table in its query.
const SOURCE: &str = "GRAPH_ALGO_SOURCE";
/// The correlation name of an edge's destination's vertex table.
const DESTINATION: &str = "GRAPH_ALGO_DESTINATION";

/// A property graph named in place of the edges, bound.
pub(super) struct Catalogued {
    /// The queries of its edge tables, then of its vertex tables.
    pub(super) tables: Vec<BoundTable>,
    /// How their rows are made the edges and the vertices.
    pub(super) shape: GraphShape,
    /// The type of a vertex: an identity's, or of SYS_GRAPH a `$NODE_ID`'s.
    pub(super) key: DataType,
    /// The names of the edges' columns as the library reads them: two
    /// ends, named by no name a column has, then the columns alike.
    pub(super) edge_columns: Vec<String>,
}

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

/// Graph `g` as the edges of `algorithm`, its element tables bound as
/// table arguments nested `depth` deep.
///
/// # Errors
///
/// SQLCODE -103 if `g` is not valid or an endpoint's vertex table is none
/// of its own; any of reading its tables.
pub(super) fn bound(b: Bounds<'_>, algorithm: Algorithm, g: &GraphDesc, depth: usize) -> Result<Catalogued> {
    if !g.valid {
        return Err(refused(algorithm, &format!("graph {} is not valid until ALTER PROPERTY GRAPH ... COMPILE finds it holds", g.name)));
    }
    let of = |kind: ElementKind| g.elements.iter().filter(move |e| e.kind == kind);
    let edge_tables: Vec<&ElementTableDesc> = of(ElementKind::Edge).filter(|e| e.endpoints.is_some()).collect();
    let alike = alike(b, &edge_tables);
    let mut tables = Vec::new();
    let mut edges = Vec::new();
    for e in &edge_tables {
        let (select, shape) = edge_select(algorithm, g, e, &alike)?;
        tables.push(argument(b, &select, depth)?);
        edges.push(shape);
    }
    let mut vertices = Vec::new();
    for v in of(ElementKind::Vertex) {
        let keys: Vec<String> = v.key.iter().map(|k| quoted(k)).collect();
        tables.push(argument(b, &format!("SELECT {} FROM {}", keys.join(", "), table(&v.table)), depth)?);
        vertices.push(v.alias.clone());
    }
    let native = g.name.ident == SYS_GRAPH;
    let key = match (native, tables.get(edges.len()).and_then(|t| t.columns.first())) {
        (true, Some(c)) => c.data_type.clone(),
        (true | false, _) => identity_type(),
    };
    let edge_columns = [String::new(), String::new()].into_iter().chain(alike.into_iter().map(|c| c.name)).collect();
    Ok(Catalogued { tables, shape: GraphShape { edges, vertices, native }, key, edge_columns })
}

/// The SELECT of edge table `e` of `g` and its rows' shape: its ends'
/// vertex tables' keys, of the rows its endpoints reference, then the
/// columns `alike`.
fn edge_select(algorithm: Algorithm, g: &GraphDesc, e: &ElementTableDesc, alike: &[ColumnDesc]) -> Result<(String, EdgeShape)> {
    let vertex = |p: &EndpointDesc| {
        g.elements
            .iter()
            .find(|v| v.alias == p.vertex && v.kind == ElementKind::Vertex)
            .ok_or_else(|| refused(algorithm, &format!("graph {} has no vertex table {}", g.name, p.vertex)))
    };
    let Some(points) = &e.endpoints else { return Err(refused(algorithm, &format!("{} has no endpoints", e.alias))) };
    let (source, destination) = (vertex(&points.source)?, vertex(&points.destination)?);
    let keys = |of: &ElementTableDesc, at: &str| of.key.iter().map(|k| format!("{}.{}", quoted(at), quoted(k))).collect::<Vec<_>>();
    let columns: Vec<String> = keys(source, SOURCE)
        .into_iter()
        .chain(keys(destination, DESTINATION))
        .chain(alike.iter().map(|c| format!("{}.{}", quoted(EDGE), quoted(&c.name))))
        .collect();
    let select = format!(
        "SELECT {} FROM {} AS {} JOIN {} AS {} ON {} JOIN {} AS {} ON {}",
        columns.join(", "),
        table(&e.table),
        quoted(EDGE),
        table(&source.table),
        quoted(SOURCE),
        joined(&points.source, SOURCE),
        table(&destination.table),
        quoted(DESTINATION),
        joined(&points.destination, DESTINATION),
    );
    let shape = EdgeShape {
        source: (source.alias.clone(), source.key.len()),
        destination: (destination.alias.clone(), destination.key.len()),
        undirected: points.undirected,
    };
    Ok((select, shape))
}

/// The condition joining an edge to the vertex, of correlation name `at`,
/// its endpoint `p` references.
fn joined(p: &EndpointDesc, at: &str) -> String {
    let pairs: Vec<String> = p
        .columns
        .iter()
        .zip(&p.references)
        .map(|(ours, theirs)| format!("{}.{} = {}.{}", quoted(EDGE), quoted(ours), quoted(at), quoted(theirs)))
        .collect();
    pairs.join(" AND ")
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

/// `name` as a `<delimited identifier>`.
fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Table `t`'s name, quoted.
fn table(t: &QualifiedName) -> String {
    format!("{}.{}", quoted(&t.auth), quoted(&t.ident))
}

/// The query `select` bound as a table argument, nested `depth` deep.
fn argument(b: Bounds<'_>, select: &str, depth: usize) -> Result<BoundTable> {
    let spec = Parser::new(select)?.cursor_specification()?;
    let proper = TableArgumentProper::Query(Box::new(Subquery { query: spec.query, at: 0 }));
    let ta = TableArgument { proper, correlation: None, columns: None, partition: None, prune: None, order: Vec::new(), at: 0 };
    table_argument(b, &ta, WHOLE, depth)
}
