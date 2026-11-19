//! \[EXTENSION\] LINE_GRAPH ( g , L ) [GS §11.3]: the graph whose vertices
//! are g's edges of label L and whose edges join each such edge to each
//! one that may follow it in a path, so a condition between consecutive
//! edges is one between adjacent vertices (RULES.md D142).
//!
//! It is made as the GRAPH_TABLE is bound and read as it is searched,
//! never stored. Each edge table of label L is a vertex table of it, of
//! its own key and label L's properties. Of each two such tables, the
//! first's end at a vertex table and the second's start there, an edge
//! table of label FOLLOWS joins them, of no properties: an element table
//! of a query's rows (§11.3), those of the two tables and the vertex
//! table, each pair of edges that meet at a vertex. An undirected edge
//! table may be followed either way, so either end may be where it meets
//! the next.

use super::grf;
use crate::catalog::{ElementKind, ElementTableDesc, EndpointDesc, Endpoints, GraphDesc, GraphMode, LabelDesc, QualifiedName, identifier};
use crate::error::Result;

/// The label of the line graph's edges.
pub(super) const FOLLOWS: &str = "FOLLOWS";

/// The two ends of an edge table one may arrive at and leave from: the
/// end entered, then the end left.
fn ways(e: &ElementTableDesc) -> Vec<(&EndpointDesc, &EndpointDesc)> {
    let Some(ends) = &e.endpoints else { return Vec::new() };
    let forward = (&ends.source, &ends.destination);
    if ends.undirected { vec![forward, (&ends.destination, &ends.source)] } else { vec![forward] }
}

/// `t` qualified, as a query names it.
fn named(t: &QualifiedName) -> String {
    format!("{}.{}", identifier(&t.auth), identifier(&t.ident))
}

/// LINE_GRAPH ( `base` , `label` ).
///
/// # Errors
///
/// SQLCODE -103 (GRF-0012) if no edge table of `base` has `label`.
pub(super) fn line_graph(base: &GraphDesc, label: &str) -> Result<GraphDesc> {
    let tables: Vec<&ElementTableDesc> = base.edges().filter(|e| e.has_label(label)).collect();
    if tables.is_empty() {
        return Err(grf("GRF-0012", format!("property graph {} has no edge label {label}", base.name)));
    }
    let mut elements: Vec<ElementTableDesc> = tables
        .iter()
        .map(|e| ElementTableDesc {
            alias: e.alias.clone(),
            table: e.table.clone(),
            kind: ElementKind::Vertex,
            key: e.key.clone(),
            labels: e.label(label).cloned().into_iter().collect(),
            endpoints: None,
            query: None,
        })
        .collect();
    for first in &tables {
        for second in &tables {
            for (n, ((_, left), (entered, _))) in
                ways(first).into_iter().flat_map(|a| ways(second).into_iter().map(move |b| (a, b))).enumerate()
            {
                if left.vertex != entered.vertex {
                    continue;
                }
                let alias = format!("{}${}${n}", first.alias, second.alias);
                elements.push(follows((first, second), (left, entered), (base, &alias))?);
            }
        }
    }
    let name = QualifiedName { auth: base.name.auth.clone(), ident: format!("LINE_GRAPH({}, {label})", base.name.ident) };
    let desc = GraphDesc { name, elements, mode: GraphMode::Trusted, select: Vec::new(), ..base.clone() };
    Ok(desc)
}

/// The edge table of the line graph from edges of `first` left at end
/// `left` to edges of `second` entered at end `entered`, both of one
/// vertex table, of a query's rows: each edge's key, the first's as S1,
/// S2, ..., the second's as D1, D2, ....
fn follows(
    (first, second): (&ElementTableDesc, &ElementTableDesc),
    (left, entered): (&EndpointDesc, &EndpointDesc),
    (base, alias): (&GraphDesc, &str),
) -> Result<ElementTableDesc> {
    let vertex = base.element(&left.vertex).ok_or_else(|| grf("GRF-0005", format!("there is no vertex table {}", left.vertex)))?;
    let starts: Vec<String> = (1..=first.key.len()).map(|i| format!("S{i}")).collect();
    let ends: Vec<String> = (1..=second.key.len()).map(|i| format!("D{i}")).collect();
    let mut items: Vec<String> = first.key.iter().zip(&starts).map(|(k, s)| format!("E1.{} AS {s}", identifier(k))).collect();
    items.extend(second.key.iter().zip(&ends).map(|(k, d)| format!("E2.{} AS {d}", identifier(k))));
    let mut on: Vec<String> =
        left.columns.iter().zip(&left.references).map(|(c, r)| format!("E1.{} = V.{}", identifier(c), identifier(r))).collect();
    on.extend(entered.columns.iter().zip(&entered.references).map(|(c, r)| format!("E2.{} = V.{}", identifier(c), identifier(r))));
    let text = format!(
        "SELECT {} FROM {} AS E1, {} AS V, {} AS E2 WHERE {}",
        items.join(", "),
        named(&first.table),
        named(&vertex.table),
        named(&second.table),
        on.join(" AND ")
    );
    let source = EndpointDesc { vertex: first.alias.clone(), columns: starts.clone(), references: first.key.clone() };
    let destination = EndpointDesc { vertex: second.alias.clone(), columns: ends.clone(), references: second.key.clone() };
    let element = ElementTableDesc {
        alias: alias.to_owned(),
        table: QualifiedName { auth: String::new(), ident: alias.to_owned() },
        kind: ElementKind::Edge,
        key: starts.into_iter().chain(ends).collect(),
        labels: vec![LabelDesc { name: FOLLOWS.to_owned(), properties: Vec::new() }],
        endpoints: Some(Endpoints { source, destination, undirected: false }),
        query: Some(text),
    };
    Ok(element)
}
