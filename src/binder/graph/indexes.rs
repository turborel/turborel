//! \[EXTENSION\] The label and reachability indexes a GRAPH_TABLE's
//! element table is read through, if it has them (RULES.md D149, D150):
//! of a base table, any index of the labels method, as each holds every
//! row; of a base edge table, an index of the reachability method of its
//! source's columns, then its destination's, split there.

use super::super::Context;
use crate::catalog::{IndexDesc, QualifiedName};
use crate::index::reachability::SOURCE_COLUMNS_OPTION;
use crate::index::{LABELS_METHOD, REACHABILITY_METHOD};
use crate::logical::{EdgeShape, ElementRead};

/// The label index of table `table`, and of an edge table of ends
/// `ends` its reachability index, if it is read where it is stored.
pub(super) fn graph_indexes(
    cx: Context<'_>,
    table: &QualifiedName,
    (read, ends): (&ElementRead, Option<&EdgeShape>),
) -> (Option<String>, Option<String>) {
    let ElementRead::Stored = read else { return (None, None) };
    let indexes = cx.db.catalog.indexes(table);
    let labels = indexes.iter().find(|ix| ix.method == LABELS_METHOD).map(|ix| ix.name.clone());
    let reach = ends.and_then(|ends| {
        let columns: Vec<usize> = ends.source.columns.iter().chain(&ends.destination.columns).copied().collect();
        let fits = |ix: &&&IndexDesc| {
            ix.method == REACHABILITY_METHOD && ix.columns == columns && source_of(ix) == Some(ends.source.columns.len())
        };
        indexes.iter().find(fits).map(|ix| ix.name.clone())
    });
    (labels, reach)
}

/// The source's columns of reachability index `ix`.
fn source_of(ix: &IndexDesc) -> Option<usize> {
    let given = ix.options.iter().find(|(n, _)| n == SOURCE_COLUMNS_OPTION).map(|(_, v)| v.to_string().parse::<usize>().ok());
    given.unwrap_or_else(|| ix.columns.len().checked_div(2))
}
