//! \[EXTENSION\] The Definition Schema's base tables of property graphs
//! (RULES.md D-GRAPH-CATALOG, §20 of the property graph implementation
//! standard): PROPERTY_GRAPHS, of each graph descriptor, its definition
//! text as `GraphDesc::definition` writes it; PG_ELEMENT_TABLES, of each
//! element table; PG_LABELS, of each label of one; PG_PROPERTIES, of each
//! property of a label; PG_KEY_COLUMNS, of each column of an element
//! table's key; PG_EDGE_ENDPOINTS, of each pair of columns that joins an
//! edge to its source or destination; and PROPERTY_GRAPH_PRIVILEGES, of
//! each SELECT on a graph, its owner's of the grantor "_SYSTEM" found from
//! what it reads. The views of graphs read them
//! (`system/information/graphs.rs`).

use std::sync::Arc;

use super::privileges::PUBLIC;
use super::{Columns, defined, ids, long};
use crate::catalog::{ColumnDesc, ElementKind, ElementTableDesc, EndpointDesc, GraphDesc, GraphMode, owner_of, sole};
use crate::storage::Row;
use crate::system::{Rows, SYSTEM_GRANTOR, Seen, SystemTable, catalog_name, number, text, yes};
use crate::value::Value;

/// The tables.
pub(super) fn tables() -> Vec<Arc<dyn SystemTable>> {
    vec![
        defined("PROPERTY_GRAPHS", graphs_columns, Rows::Infallible(graphs)),
        defined("PG_ELEMENT_TABLES", elements_columns, Rows::Infallible(elements)),
        defined("PG_LABELS", labels_columns, Rows::Infallible(labels)),
        defined("PG_PROPERTIES", properties_columns, Rows::Infallible(properties)),
        defined("PG_KEY_COLUMNS", keys_columns, Rows::Infallible(keys)),
        defined("PG_EDGE_ENDPOINTS", endpoints_columns, Rows::Infallible(endpoints)),
        defined("PROPERTY_GRAPH_PRIVILEGES", privileges_columns, Rows::Infallible(privileges)),
    ]
}

/// Each element table of each graph, with its graph's catalog, schema and
/// name before the rest of its row, `of` gives.
fn each_element(seen: &Seen<'_>, of: &dyn Fn(&GraphDesc, &ElementTableDesc) -> Vec<Vec<Value>>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for g in seen.state.catalog.graphs() {
        for e in &g.elements {
            rows.extend(of(g, e).into_iter().map(|tail| [ids(&catalog, &g.name).to_vec(), vec![text(&e.alias)], tail].concat()));
        }
    }
    rows
}

// ---------------------------------------------------------------------------
// PROPERTY_GRAPHS
// ---------------------------------------------------------------------------

fn graphs_columns() -> Vec<ColumnDesc> {
    Columns::of()
        .ids("PROPERTY_GRAPH", true)
        .data("PROPERTY_GRAPH_MODE", true)
        .data("IS_INVOKER_RIGHTS", true)
        .data("IS_VALID", true)
        .data("PROPERTY_GRAPH_DEFINITION", false)
        .done()
}

/// Each graph: its mode, whether WITH INVOKER RIGHTS, whether valid, and
/// its definition, null if longer than CHARACTER_DATA holds.
fn graphs(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let row = |g: &GraphDesc| {
        let mode = match g.mode {
            GraphMode::Enforced => "ENFORCED",
            GraphMode::Trusted => "TRUSTED",
        };
        let tail = vec![text(mode), yes(g.invoker), yes(g.valid), long(&g.definition())];
        [ids(&catalog, &g.name).to_vec(), tail].concat()
    };
    seen.state.catalog.graphs().into_iter().map(row).collect()
}

// ---------------------------------------------------------------------------
// PG_ELEMENT_TABLES, PG_LABELS, PG_PROPERTIES, PG_KEY_COLUMNS
// ---------------------------------------------------------------------------

fn elements_columns() -> Vec<ColumnDesc> {
    Columns::of()
        .ids("PROPERTY_GRAPH", true)
        .id("ELEMENT_TABLE_ALIAS", true)
        .data("ELEMENT_TABLE_KIND", true)
        .ids("TABLE", true)
        .number("ORDINAL_POSITION", true)
        .data("IS_UNDIRECTED", true)
        .done()
}

/// Each element table: VERTEX or EDGE, its table, its place among its
/// graph's of its kind, and whether its edges are undirected.
fn elements(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    each_element(seen, &|g, e| {
        let kind = match e.kind {
            ElementKind::Vertex => "VERTEX",
            ElementKind::Edge => "EDGE",
        };
        let place = g.elements.iter().filter(|d| d.kind == e.kind).position(|d| d.alias == e.alias).unwrap_or_default();
        let undirected = e.endpoints.as_ref().is_some_and(|ends| ends.undirected);
        vec![[vec![text(kind)], ids(&catalog, &e.table).to_vec(), vec![number(place.saturating_add(1)), yes(undirected)]].concat()]
    })
}

fn labels_columns() -> Vec<ColumnDesc> {
    Columns::of().ids("PROPERTY_GRAPH", true).id("ELEMENT_TABLE_ALIAS", true).id("LABEL_NAME", true).number("ORDINAL_POSITION", true).done()
}

/// Each label of each element table, in order.
fn labels(seen: &Seen<'_>) -> Vec<Row> {
    each_element(seen, &|_, e| e.labels.iter().enumerate().map(|(k, l)| vec![text(&l.name), number(k.saturating_add(1))]).collect())
}

fn properties_columns() -> Vec<ColumnDesc> {
    Columns::of()
        .ids("PROPERTY_GRAPH", true)
        .id("ELEMENT_TABLE_ALIAS", true)
        .id("LABEL_NAME", true)
        .id("PROPERTY_NAME", true)
        .number("ORDINAL_POSITION", true)
        .data("PROPERTY_EXPRESSION", false)
        .data("DATA_TYPE", true)
        .done()
}

/// Each property of each label, in order: its expression, null if longer
/// than CHARACTER_DATA holds, and its declared type.
fn properties(seen: &Seen<'_>) -> Vec<Row> {
    each_element(seen, &|_, e| {
        let mut rows = Vec::new();
        for l in &e.labels {
            for (k, p) in l.properties.iter().enumerate() {
                let tail = vec![number(k.saturating_add(1)), long(&p.expression), text(&p.data_type.to_string())];
                rows.push([vec![text(&l.name), text(&p.name)], tail].concat());
            }
        }
        rows
    })
}

fn keys_columns() -> Vec<ColumnDesc> {
    Columns::of()
        .ids("PROPERTY_GRAPH", true)
        .id("ELEMENT_TABLE_ALIAS", true)
        .id("COLUMN_NAME", true)
        .number("ORDINAL_POSITION", true)
        .done()
}

/// Each column of each element table's key, in order.
fn keys(seen: &Seen<'_>) -> Vec<Row> {
    each_element(seen, &|_, e| e.key.iter().enumerate().map(|(k, c)| vec![text(c), number(k.saturating_add(1))]).collect())
}

// ---------------------------------------------------------------------------
// PG_EDGE_ENDPOINTS
// ---------------------------------------------------------------------------

fn endpoints_columns() -> Vec<ColumnDesc> {
    Columns::of()
        .ids("PROPERTY_GRAPH", true)
        .id("EDGE_TABLE_ALIAS", true)
        .data("EDGE_END", true)
        .id("VERTEX_TABLE_ALIAS", true)
        .number("ORDINAL_POSITION", true)
        .id("EDGE_COLUMN_NAME", true)
        .id("VERTEX_COLUMN_NAME", true)
        .done()
}

/// Of each edge table, each column of its source's key and of its
/// destination's, with the vertex table's column it matches.
fn endpoints(seen: &Seen<'_>) -> Vec<Row> {
    let pairs = |end: &str, e: &EndpointDesc| -> Vec<Vec<Value>> {
        let each = e.columns.iter().zip(&e.references).enumerate();
        each.map(|(k, (ours, theirs))| vec![text(end), text(&e.vertex), number(k.saturating_add(1)), text(ours), text(theirs)]).collect()
    };
    each_element(seen, &|_, e| match &e.endpoints {
        Some(ends) => [pairs("SOURCE", &ends.source), pairs("DESTINATION", &ends.destination)].concat(),
        None => Vec::new(),
    })
}

// ---------------------------------------------------------------------------
// PROPERTY_GRAPH_PRIVILEGES
// ---------------------------------------------------------------------------

fn privileges_columns() -> Vec<ColumnDesc> {
    Columns::of()
        .id("GRANTOR", true)
        .id("GRANTEE", true)
        .ids("PROPERTY_GRAPH", true)
        .data("PRIVILEGE_TYPE", true)
        .data("IS_GRANTABLE", true)
        .done()
}

/// Each SELECT on each graph: its owner's first, then those granted.
fn privileges(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let all = &*seen.state.catalog;
    let mut rows = Vec::new();
    for g in all.graphs() {
        let owner = owner_of(all, &g.name.auth);
        let row = |grantor: &str, grantee: &str, grantable: bool| {
            [vec![text(grantor), text(grantee)], ids(&catalog, &g.name).to_vec(), vec![text("SELECT"), yes(grantable)]].concat()
        };
        rows.push(row(SYSTEM_GRANTOR, owner, g.select_of(all, sole(all, owner)).1));
        rows.extend(g.select.iter().map(|u| row(&u.grantor, u.grantee.as_deref().unwrap_or(PUBLIC), u.grantable)));
    }
    rows
}
