//! \[EXTENSION\] The Definition Schema's base tables of Mode C of the
//! property graph implementation standard (§4.3, RULES.md D-GQL):
//! GRAPH_TYPES, of each graph type, its definition text; and GQL_GRAPHS,
//! of each GQL graph, the graph type it is of, null if it is open. The
//! views read them (`system/information/graph_types.rs`).

use std::sync::Arc;

use super::{Columns, defined, ids, long};
use crate::catalog::{ColumnDesc, GqlGraphDesc, GraphTypeDesc};
use crate::storage::Row;
use crate::system::{Rows, Seen, SystemTable, catalog_name, text};
use crate::value::Value;

/// The tables.
pub(super) fn tables() -> Vec<Arc<dyn SystemTable>> {
    vec![defined("GRAPH_TYPES", types_columns, Rows::Infallible(types)), defined("GQL_GRAPHS", graphs_columns, Rows::Infallible(graphs))]
}

fn types_columns() -> Vec<ColumnDesc> {
    Columns::of().ids("GRAPH_TYPE", true).data("GRAPH_TYPE_DEFINITION", false).done()
}

/// Each graph type, and its definition, null if longer than
/// CHARACTER_DATA holds.
fn types(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let row = |t: &GraphTypeDesc| [ids(&catalog, &t.name).to_vec(), vec![long(&t.definition)]].concat();
    seen.state.catalog.gql().graph_types().into_iter().map(row).collect()
}

fn graphs_columns() -> Vec<ColumnDesc> {
    Columns::of().ids("GRAPH", true).data("GRAPH_TYPE_SCHEMA", false).data("GRAPH_TYPE_NAME", false).done()
}

/// Each GQL graph, and its graph type's schema and name, null if open.
fn graphs(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let row = |g: &GqlGraphDesc| {
        let typing = g.graph_type.as_ref().map_or_else(|| vec![Value::Null, Value::Null], |t| vec![text(&t.auth), text(&t.ident)]);
        [ids(&catalog, &g.name).to_vec(), typing].concat()
    };
    seen.state.catalog.gql().gql_graphs().into_iter().map(row).collect()
}
