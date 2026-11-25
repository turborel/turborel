//! \[EXTENSION\] [ISO/IEC 9075:1992 §21.1] INFORMATION_SCHEMA.COLUMN_STORAGE
//! (RULES.md D1165): what each column of each base table takes, by the way
//! its values are kept, as TABLE_STATISTICS shows what the table takes
//! (`TableStorage::sizes`).
//!
//! A row is of a column and an encoding. Of a column store's frozen
//! groups, it is the chunks of one encoding (FIXED, VARYING, CONSTANT,
//! FRAME, DICTIONARY, RUNS, ALP, FSST), as each chunk's first byte names
//! it: their count, the rows they hold, their bytes encoded, and the bytes
//! of the pages they take, a page each of a chunk on a page of its own.
//! Reading the view reads each chunk, so it reads every group's pages. Of a tree's rows, of either store, it is
//! ROW: the rows, and their number times the column's mean width the
//! table's statistics last measured, NULL if they never did; no chunks and
//! no pages, as a column's values share the tree's pages with the others'.
//! A partitioned table's are those of its partitions, one after another.
//! A table whose engine does not know its sizes has no row.
//!
//! The database's owner, and a user granted the role MONITOR, see every
//! column's rows; any other user those of the columns on which it holds a
//! privilege, as COLUMN_STATISTICS shows them.

use std::sync::Arc;

use super::column_stats::on_column;
use super::sizes::{maybe, named, shown, sizes_of};
use super::statements::{count, counted};
use super::{Seen, SystemTable, cardinal, identifier, number, text};
use crate::catalog::{ColumnDesc, Who, applicable};
use crate::error::Result;
use crate::storage::Row;
use crate::value::Value;

/// COLUMN_STORAGE.
struct ColumnStorage;

/// The table.
pub(super) fn tables() -> [Arc<dyn SystemTable>; 1] {
    [Arc::new(ColumnStorage)]
}

impl SystemTable for ColumnStorage {
    fn name(&self) -> &'static str {
        "COLUMN_STORAGE"
    }

    fn columns(&self) -> Vec<ColumnDesc> {
        let mut columns: Vec<ColumnDesc> =
            ["TABLE_CATALOG", "TABLE_SCHEMA", "TABLE_NAME", "COLUMN_NAME"].iter().map(|n| identifier(n, true)).collect();
        columns.push(cardinal("ORDINAL_POSITION", true));
        columns.push(identifier("ENCODING", false));
        columns.push(ColumnDesc { not_null: false, ..counted("CHUNKS") });
        columns.push(counted("ROW_COUNT"));
        columns.extend(["BYTES", "PAGE_BYTES"].map(|n| ColumnDesc { not_null: false, ..counted(n) }));
        columns
    }

    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        let every = seen.may_monitor();
        let who = Who::sole(seen.state.catalog.roles(), seen.auth);
        let mut rows = Vec::new();
        for t in shown(seen) {
            let held = applicable(&*seen.state.catalog, who, &t.name).0;
            let leaves = seen.state.catalog.partitioning().leaves(&t.name);
            let Some(sizes) = sizes_of(seen, &leaves, true) else { continue };
            for c in sizes.columns.iter().filter(|c| every || on_column(&held, c.column)) {
                let Some(column) = t.columns.get(c.column) else { continue };
                let mut row = named(seen, &t.name);
                row.extend([text(&column.name), number(c.column.saturating_add(1))]);
                row.push(c.encoding.map_or(Value::Null, text));
                row.extend([maybe(c.chunks), count(c.rows), maybe(c.bytes), maybe(c.page_bytes)]);
                rows.push(row);
            }
        }
        Ok(rows)
    }
}
