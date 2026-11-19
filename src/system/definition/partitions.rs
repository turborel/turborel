//! \[EXTENSION\] The tables of partitioning (RULES.md D153, D158), which
//! the views INFORMATION_SCHEMA.PARTITIONED_TABLES, PARTITIONS and
//! PARTITION_INDEXES read. PARTITIONED_TABLES has a row of each
//! partitioned table: its strategy, its key's parts as written, and how
//! many partitions it has one level down. PARTITIONS has a row of each
//! partition: its table, the table it is a partition of, its bound as
//! written, its level, the root's level 0, and whether it holds rows.
//! PARTITION_INDEXES has a row of each index a template made: the
//! template, the partitioned table that keeps it, and the index and the
//! partition it is on.

use std::sync::Arc;

use super::{Columns, defined, ids, long};
use crate::catalog::{ColumnDesc, EXPRESSION_PART, PartitionDesc, derived_index_name};
use crate::storage::Row;
use crate::system::{Rows, Seen, SystemTable, catalog_name, number, text, yes};

/// The tables.
pub(super) fn tables() -> Vec<Arc<dyn SystemTable>> {
    vec![
        defined("PARTITIONED_TABLES", partitioned_columns, Rows::Infallible(partitioned)),
        defined("PARTITIONS", partitions_columns, Rows::Infallible(partitions)),
        defined("PARTITION_INDEXES", indexes_columns, Rows::Infallible(indexes)),
    ]
}

fn partitioned_columns() -> Vec<ColumnDesc> {
    Columns::of().ids("TABLE", true).id("STRATEGY", true).data("KEY_PARTS", false).number("PARTITION_COUNT", true).done()
}

fn partitions_columns() -> Vec<ColumnDesc> {
    Columns::of()
        .ids("TABLE", true)
        .ids("PARENT_TABLE", true)
        .data("BOUND", false)
        .number("PARTITION_LEVEL", true)
        .id("IS_PARTITIONED", true)
        .done()
}

fn indexes_columns() -> Vec<ColumnDesc> {
    Columns::of()
        .ids("TABLE", true)
        .id("TEMPLATE_NAME", true)
        .id("METHOD_NAME", true)
        .id("IS_UNIQUE", true)
        .ids("PARTITION", true)
        .id("INDEX_NAME", true)
        .done()
}

/// Each partitioned table and table partitioning describes, in order of
/// name.
fn described<'a>(seen: &Seen<'a>) -> Vec<&'a PartitionDesc> {
    let mut all: Vec<&PartitionDesc> = seen.state.catalog.partitioning().all().collect();
    all.sort_by(|a, b| (&a.table.auth, &a.table.ident).cmp(&(&b.table.auth, &b.table.ident)));
    all
}

/// Each partitioned table.
fn partitioned(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let p = seen.state.catalog.partitioning();
    let mut rows = Vec::new();
    for d in described(seen) {
        let (Some(key), Some(t)) = (&d.key, seen.state.catalog.table(&d.table)) else { continue };
        let parts: Vec<String> = (0..key.columns.len())
            .filter_map(|at| match key.expressions.iter().find(|(part, _)| *part == at) {
                Some((_, written)) => Some(format!("({written})")),
                None => key.columns.get(at).filter(|&&c| c != EXPRESSION_PART).and_then(|&c| t.columns.get(c)).map(|col| col.name.clone()),
            })
            .collect();
        let mut row: Row = ids(&catalog, &d.table).to_vec();
        row.extend([text(&key.strategy), long(&parts.join(", ")), number(p.children(&d.table).len())]);
        rows.push(row);
    }
    rows
}

/// Each partition.
fn partitions(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let p = seen.state.catalog.partitioning();
    let mut rows = Vec::new();
    for d in described(seen) {
        let Some(of) = &d.of else { continue };
        let mut row: Row = ids(&catalog, &d.table).to_vec();
        row.extend(ids(&catalog, &of.parent));
        row.extend([long(&of.bound.text()), number(p.ancestors(&d.table).len()), yes(d.key.is_some())]);
        rows.push(row);
    }
    rows
}

/// Each index a template made, of each template.
fn indexes(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let p = seen.state.catalog.partitioning();
    let mut rows = Vec::new();
    for d in described(seen) {
        let Some(key) = &d.key else { continue };
        for t in &key.templates {
            for leaf in p.leaves(&d.table) {
                let made = derived_index_name(&t.name, &leaf);
                if seen.state.catalog.index(&made).is_none() || seen.state.building.contains(&made) {
                    continue;
                }
                let mut row: Row = ids(&catalog, &d.table).to_vec();
                row.extend([text(&t.name), text(&t.method), yes(t.unique)]);
                row.extend(ids(&catalog, &leaf));
                row.push(text(&made));
                rows.push(row);
            }
        }
    }
    rows
}
