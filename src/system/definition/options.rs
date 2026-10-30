//! \[EXTENSION\] [ISO/IEC 9075:1992 §21.1] The Definition Schema's table of
//! table options (RULES.md D41), TABLE_OPTIONS: for each base table, each
//! option its engine or a plug-in declares, with who declared it, the value
//! the table has, whether the table set it, its default, the values it
//! takes, how a change of it is made, and what it does; and of indexes
//! (RULES.md D34): INDEXES, each index with its method and columns, and
//! INDEX_OPTIONS, each option of each index's method, as TABLE_OPTIONS
//! shows a table's. Split from definition.rs (RULES.md R-SIZE-2).

use std::sync::Arc;

use super::{Columns, defined, ids, long};
use crate::catalog::{ColumnDesc, IndexDesc, TableDesc};
use crate::storage::{Rebuild, Row, TableOption, option_value};
use crate::system::{Rows, Seen, SystemTable, catalog_name, text, yes};
use crate::value::Value;

/// The tables.
pub(super) fn tables() -> Vec<Arc<dyn SystemTable>> {
    vec![
        defined("TABLE_OPTIONS", table_options_columns, Rows::Infallible(table_options)),
        defined("INDEXES", indexes_columns, Rows::Infallible(indexes)),
        defined("INDEX_OPTIONS", index_options_columns, Rows::Infallible(index_options)),
    ]
}

fn indexes_columns() -> Vec<ColumnDesc> {
    Columns::of()
        .ids("TABLE", true)
        .id("INDEX_NAME", true)
        .id("METHOD_NAME", true)
        .data("IS_UNIQUE", true)
        .data("COLUMN_NAMES", false)
        .done()
}

/// The indexes of the base tables, in order of table and name.
fn indexes(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for (t, ix) in described_indexes(seen) {
        let columns: Vec<&str> = ix.columns.iter().filter_map(|&c| t.columns.get(c)).map(|c| c.name.as_str()).collect();
        let mut row: Row = ids(&catalog, &t.name).to_vec();
        row.extend([text(&ix.name), text(&ix.method), yes(ix.unique), long(&columns.join(", "))]);
        rows.push(row);
    }
    rows
}

/// Each index with its table, in order of table and name.
fn described_indexes<'a>(seen: &Seen<'a>) -> Vec<(&'a TableDesc, &'a IndexDesc)> {
    let mut tables = seen.state.catalog.tables();
    tables.sort_by(|a, b| (&a.name.auth, &a.name.ident).cmp(&(&b.name.auth, &b.name.ident)));
    let mut out = Vec::new();
    for t in tables {
        // An index being built, or built again, is not one yet.
        let mut of: Vec<_> = seen.state.catalog.indexes(&t.name).into_iter().filter(|ix| !seen.state.building.contains(&ix.name)).collect();
        of.sort_by(|a, b| a.name.cmp(&b.name));
        out.extend(of.into_iter().map(|ix| (t, ix)));
    }
    out
}

fn index_options_columns() -> Vec<ColumnDesc> {
    Columns::of()
        .ids("TABLE", true)
        .id("INDEX_NAME", true)
        .id("METHOD_NAME", true)
        .id("OPTION_NAME", true)
        .id("DECLARED_BY", false)
        .data("OPTION_VALUE", false)
        .data("VALUE_SOURCE", true)
        .data("DEFAULT_VALUE", false)
        .data("VALUE_KIND", true)
        .data("CHANGED_BY", false)
        .data("DESCRIPTION", false)
        .done()
}

/// Each option of each index, in order of table, index and name: its
/// method's and the plug-ins', and any it sets that none declares.
fn index_options(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let mut rows = Vec::new();
    for (t, ix) in described_indexes(seen) {
        let own = seen.state.methods.get(&ix.method).map(|m| m.index_options()).unwrap_or_default();
        let declared = own.into_iter().map(|o| (ix.method.clone(), o)).chain(seen.state.index_options.iter().cloned());
        for (name, rest) in described(&declared.collect::<Vec<_>>(), &ix.options, ("INDEX", "REBUILD")) {
            let mut row: Row = ids(&catalog, &t.name).to_vec();
            row.extend([text(&ix.name), text(&ix.method), text(&name)]);
            row.extend(rest);
            rows.push(row);
        }
    }
    rows
}

fn table_options_columns() -> Vec<ColumnDesc> {
    Columns::of()
        .ids("TABLE", true)
        .id("ENGINE_NAME", true)
        .id("OPTION_NAME", true)
        .id("DECLARED_BY", false)
        .data("OPTION_VALUE", false)
        .data("VALUE_SOURCE", true)
        .data("DEFAULT_VALUE", false)
        .data("VALUE_KIND", true)
        .data("CHANGED_BY", false)
        .data("DESCRIPTION", false)
        .done()
}

/// Each option of each base table, in order of table and name: its
/// engine's and the plug-ins', and any it sets that none declares, of a
/// plug-in not registered since the database was opened.
fn table_options(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let mut tables = seen.state.catalog.tables();
    tables.sort_by(|a, b| (&a.name.auth, &a.name.ident).cmp(&(&b.name.auth, &b.name.ident)));
    let mut rows = Vec::new();
    for t in tables {
        let own = seen.state.engines.get(&t.engine).map(|e| e.table_options()).unwrap_or_default();
        let declared = own.into_iter().map(|o| (t.engine.clone(), o)).chain(seen.state.table_options.iter().cloned());
        for (name, rest) in described(&declared.collect::<Vec<_>>(), &t.options, ("TABLE", "REBUILD")) {
            let mut row: Row = ids(&catalog, &t.name).to_vec();
            row.extend([text(&t.engine), text(&name)]);
            row.extend(rest);
            rows.push(row);
        }
    }
    rows
}

/// The options `declared`, each with who declared it, of a table or index
/// that sets `set`, and those it sets that none declares, in order of name:
/// each its name, then who declared it, its value, its source (`own`, the
/// table's or index's, or DEFAULT), default, kind, how a change is made (in
/// place, or `rebuilt`), and what it does.
fn described(declared: &[(String, TableOption)], set: &[(String, Value)], (own, rebuilt): (&str, &str)) -> Vec<(String, Row)> {
    let options: Vec<TableOption> = declared.iter().map(|(_, o)| o.clone()).collect();
    let mut named: Vec<(String, Row)> = Vec::new();
    for (by, o) in declared {
        let value = option_value(&options, set, &o.name).map_or(Value::Null, |v| long(&v.to_string()));
        let source = if set.iter().any(|(n, _)| *n == o.name) { own } else { "DEFAULT" };
        let changed = match o.rebuild {
            Rebuild::No => "IN PLACE",
            Rebuild::Yes => rebuilt,
        };
        let row = vec![text(by), value, text(source), long(&o.default.to_string()), text(&o.kind.described()), text(changed), long(&o.doc)];
        named.push((o.name.clone(), row));
    }
    for (n, v) in set.iter().filter(|(n, _)| !options.iter().any(|o| o.name == *n)) {
        let row = vec![Value::Null, long(&v.to_string()), text(own), Value::Null, text("NOT DECLARED"), Value::Null, Value::Null];
        named.push((n.clone(), row));
    }
    named.sort_by(|a, b| a.0.cmp(&b.0));
    named
}
