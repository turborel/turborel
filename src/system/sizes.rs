//! \[EXTENSION\] [ISO/IEC 9075:1992 §21.1] INFORMATION_SCHEMA.TABLE_STATISTICS
//! and INDEX_STATISTICS (RULES.md D522): what each base table holds and
//! takes, and how each index is used, as the database knows it now.
//!
//! TABLE_STATISTICS has a row for each base table: its engine, its rows,
//! the bytes it takes (`TableStorage::sizes`), whether some of them are
//! estimated, when a transaction that wrote it last committed, and when its
//! engine's maintenance last did some work on it, each kept by the table's
//! lock (`txn/locks.rs`) since the database was opened. A partitioned
//! table's are those of its partitions together.
//!
//! INDEX_STATISTICS has a row for each index: its method, the searches
//! made of it and the entries they gave (`index/uses.rs`) since the
//! database was opened or the index made, and what its method knows of
//! its entries, height and bytes (`Index::sizes`), and the distinct keys
//! it counts, as the planner reads them (`State::distinct_keys`, RULES.md
//! D992).
//!
//! The database's owner, and a user granted the role MONITOR the owner
//! created (`Seen::may_monitor`, RULES.md D520), see every table's and
//! index's row; any other user those of the tables on which it holds a
//! privilege, as the views of the Information Schema show what is
//! "accessible to" it.

use std::sync::Arc;

use super::column_stats::whole;
use super::statements::{count, counted, time, time_column};
use super::{Seen, SystemTable, catalog_name, identifier, text, text_column, yes};
use crate::catalog::{ColumnDesc, IndexDesc, QualifiedName, TableDesc, Who, applicable};
use crate::error::Result;
use crate::storage::{Row, TableSizes};
use crate::value::Value;

/// The longest YES or NO.
const YES_LENGTH: u32 = 3;

/// TABLE_STATISTICS.
struct TableStatistics;

/// INDEX_STATISTICS.
struct IndexStatistics;

/// The tables.
pub(super) fn tables() -> [Arc<dyn SystemTable>; 2] {
    [Arc::new(TableStatistics), Arc::new(IndexStatistics)]
}

/// The base tables `seen` shows, in order of name.
pub(super) fn shown<'a>(seen: &Seen<'a>) -> Vec<&'a TableDesc> {
    let every = seen.may_monitor();
    let who = Who::sole(seen.state.catalog.roles(), seen.auth);
    let mut tables: Vec<&TableDesc> =
        seen.state.catalog.tables().into_iter().filter(|t| every || !applicable(&*seen.state.catalog, who, &t.name).0.is_empty()).collect();
    tables.sort_by(|a, b| (&a.name.auth, &a.name.ident).cmp(&(&b.name.auth, &b.name.ident)));
    tables
}

/// The three columns naming a table.
fn table_columns() -> Vec<ColumnDesc> {
    ["TABLE_CATALOG", "TABLE_SCHEMA", "TABLE_NAME"].iter().map(|n| identifier(n, true)).collect()
}

/// The three values naming `name`.
pub(super) fn named(seen: &Seen<'_>, name: &QualifiedName) -> Row {
    vec![catalog_name(seen), text(&name.auth), text(&name.ident)]
}

/// A count, or the null value.
pub(super) fn maybe(n: Option<u64>) -> Value {
    n.map_or(Value::Null, count)
}

/// A time, an instant the clock read, or the null value.
fn when(at: Option<i64>) -> Value {
    at.map_or(Value::Null, |unix| time(crate::value::datetime::utc_of_unix(unix)))
}

/// The bytes of `leaves`, the partitions of a table or the table alone,
/// none if one does not know its own.
pub(super) fn sizes_of(seen: &Seen<'_>, leaves: &[QualifiedName], columns: bool) -> Option<TableSizes> {
    let mut all = TableSizes::default();
    for leaf in leaves {
        let own = seen.state.storage_of(leaf).ok()?.sizes(columns)?;
        all.bytes = all.bytes.saturating_add(own.bytes);
        all.estimated |= own.estimated;
        all.columns.extend(own.columns);
    }
    Some(all)
}

/// The latest of `leaves`' stamps: when each was last changed, and last
/// compacted.
fn stamps_of(seen: &Seen<'_>, leaves: &[QualifiedName]) -> (Option<i64>, Option<i64>) {
    let stamps = leaves.iter().filter_map(|leaf| seen.state.lock_of(leaf).ok().map(|lock| lock.stamps()));
    stamps.fold((None, None), |(c, m), (changed, compacted)| (c.max(changed), m.max(compacted)))
}

impl SystemTable for TableStatistics {
    fn name(&self) -> &'static str {
        "TABLE_STATISTICS"
    }

    fn columns(&self) -> Vec<ColumnDesc> {
        let mut columns = table_columns();
        columns.push(identifier("ENGINE_NAME", true));
        columns.push(counted("ROW_COUNT"));
        columns.push(ColumnDesc { not_null: false, ..counted("BYTES") });
        columns.push(text_column(("BYTES_ESTIMATED", YES_LENGTH), false));
        columns.push(time_column("LAST_CHANGED"));
        columns.push(time_column("LAST_COMPACTED"));
        columns
    }

    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        let mut rows = Vec::new();
        for t in shown(seen) {
            let leaves = seen.state.catalog.partitioning().leaves(&t.name);
            let live = seen.state.row_count(&t.name).and_then(|n| u64::try_from(n).ok()).unwrap_or(0);
            let sizes = sizes_of(seen, &leaves, false);
            let (changed, compacted) = stamps_of(seen, &leaves);
            let mut row = named(seen, &t.name);
            row.extend([text(&t.engine), count(live), maybe(sizes.as_ref().map(|s| s.bytes))]);
            row.extend([sizes.map_or(Value::Null, |s| yes(s.estimated)), when(changed), when(compacted)]);
            rows.push(row);
        }
        Ok(rows)
    }
}

impl SystemTable for IndexStatistics {
    fn name(&self) -> &'static str {
        "INDEX_STATISTICS"
    }

    fn columns(&self) -> Vec<ColumnDesc> {
        let mut columns = table_columns();
        columns.extend([identifier("INDEX_NAME", true), identifier("METHOD_NAME", true)]);
        columns.extend(["SEARCHES", "ENTRIES_READ"].into_iter().map(counted));
        let known = ["ENTRY_COUNT", "HEIGHT", "BYTES", "DISTINCT_KEYS"];
        columns.extend(known.into_iter().map(|n| ColumnDesc { not_null: false, ..counted(n) }));
        columns
    }

    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        let mut rows = Vec::new();
        for t in shown(seen) {
            let mut of: Vec<&IndexDesc> = seen.state.catalog.indexes(&t.name);
            // An index being built, or built again, is not one yet.
            of.retain(|ix| !seen.state.building.contains(&ix.name));
            of.sort_by(|a, b| a.name.cmp(&b.name));
            for ix in of {
                let (searches, entries) = seen.state.uses_of(&ix.name).map_or((0, 0), |uses| uses.totals());
                let index = seen.state.index_of(&ix.name).ok();
                let sizes = index.map(|index| index.sizes()).unwrap_or_default();
                let keys = whole(index.and_then(|index| seen.state.distinct_keys(&t.name, &**index)));
                let mut row = named(seen, &t.name);
                row.extend([text(&ix.name), text(&ix.method), count(searches), count(entries)]);
                row.extend([maybe(sizes.entries), maybe(sizes.height), maybe(sizes.bytes), keys]);
                rows.push(row);
            }
        }
        Ok(rows)
    }
}
