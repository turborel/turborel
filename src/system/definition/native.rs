//! \[EXTENSION\] The Definition Schema's base tables of node and edge
//! tables (§4.2, RULES.md D109): GRAPH_TABLES, of each node or
//! edge table, which it is; and CONNECTION_CONSTRAINTS, of each clause of
//! each CONNECTION constraint, the node tables it joins and what a
//! node's deletion does to the edges. Their views read them
//! (`system/information/native.rs`).

use std::sync::Arc;

use super::{Columns, defined, ids};
use crate::catalog::{ColumnDesc, NativeKind};
use crate::storage::Row;
use crate::system::{Rows, Seen, SystemTable, catalog_name, number, text};

/// The tables.
pub(super) fn tables() -> Vec<Arc<dyn SystemTable>> {
    vec![
        defined("GRAPH_TABLES", graph_tables_columns, Rows::Infallible(graph_tables)),
        defined("CONNECTION_CONSTRAINTS", connections_columns, Rows::Infallible(connections)),
    ]
}

fn graph_tables_columns() -> Vec<ColumnDesc> {
    Columns::of().ids("TABLE", true).data("GRAPH_TABLE_TYPE", true).done()
}

/// Each node or edge table: NODE or EDGE.
fn graph_tables(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let row = |(name, kind): (&crate::catalog::QualifiedName, NativeKind)| {
        let kind = match kind {
            NativeKind::Node => "NODE",
            NativeKind::Edge => "EDGE",
        };
        [ids(&catalog, name).to_vec(), vec![text(kind)]].concat()
    };
    seen.state.catalog.tables().into_iter().filter_map(|t| Some((&t.name, t.native()?))).map(row).collect()
}

fn connections_columns() -> Vec<ColumnDesc> {
    Columns::of()
        .ids("CONSTRAINT", true)
        .ids("TABLE", true)
        .number("ORDINAL_POSITION", true)
        .ids("FROM_TABLE", true)
        .ids("TO_TABLE", true)
        .data("DELETE_RULE", true)
        .done()
}

/// Each clause of each CONNECTION constraint, at its position, from 1:
/// the node tables it joins, and CASCADE or NO ACTION.
fn connections(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for t in seen.state.catalog.tables() {
        for (name, k) in t.connections() {
            let rule = text(if k.cascade { "CASCADE" } else { "NO ACTION" });
            for (at, (from, to)) in k.clauses.iter().enumerate() {
                let ends = [ids(&catalog, from), ids(&catalog, to)].concat();
                let head = [ids(&catalog, name).to_vec(), ids(&catalog, &t.name).to_vec(), vec![number(at.saturating_add(1))]].concat();
                rows.push([head, ends, vec![rule.clone()]].concat());
            }
        }
    }
    rows
}
