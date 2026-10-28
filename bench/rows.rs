//! How fast the row store keeps rows (RULES.md D31): inserts, fetches by
//! row id, and scans of every column and of one, of short rows (about 40
//! bytes stored) and of rows with a 4 KiB value out of line, in a buffer
//! pool that holds the table. Run with `cargo xtask bench`, on the harness
//! (RULES.md R-BENCH-1).

#![allow(
    clippy::unwrap_used,
    clippy::as_conversions,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    missing_docs,
    reason = "benchmark code: a failure ends the run (RULES.md R-DEV-3)"
)]

mod harness;

use std::sync::Arc;

use harness::{Bench, Case, scatter};
use turborel::catalog::{ColumnDesc, QualifiedName, TableDesc};
use turborel::engine::rows::{RowStore, TableFiles, TableOptions};
use turborel::log::MemoryLog;
use turborel::storage::{Row, StorageEngine, TableStorage};
use turborel::txn::NO_TRANSACTION;
use turborel::value::{DataType, Decimal, Value};

/// Short rows per table, and frames: enough for the whole table.
const ROWS: u64 = 200_000;
const FRAMES: usize = 65_536;

fn main() {
    let bench = Bench::new("rows");
    for (kind, long) in [("short rows", 0), ("4 KiB out of line", 4096)] {
        let rows = if long > 0 { ROWS / 10 } else { ROWS };
        let made: Vec<Row> = (0..rows).map(|n| row(n, long)).collect();
        bench.measure_fresh(
            Case::new(format!("insert, {kind}"), "rows"),
            || (table(&bench, long), made.clone()),
            |((_store, table), made)| {
                for r in made {
                    table.insert(NO_TRANSACTION, r).unwrap();
                }
                rows
            },
        );
        let (_store, table) = table(&bench, long);
        let ids: Vec<u64> = made.into_iter().map(|r| table.insert(NO_TRANSACTION, r).unwrap()).collect();
        bench.measure(Case::new(format!("get by row id, scattered, {kind}"), "rows"), || {
            for n in 0..rows {
                table.get(NO_TRANSACTION, ids[scatter(n, rows) as usize]).unwrap().unwrap();
            }
            rows
        });
        bench.measure(Case::new(format!("scan, every column, {kind}"), "rows"), || {
            Arc::clone(&table).scan(NO_TRANSACTION, &[0, 1, 2, 3]).count() as u64
        });
        bench.measure(Case::new(format!("scan, one number, {kind}"), "rows"), || {
            Arc::clone(&table).scan(NO_TRANSACTION, &[0]).count() as u64
        });
    }
}

/// The length of the last column: long enough for `long` bytes.
fn last_length(long: usize) -> usize {
    if long > 0 { 5000 } else { 8 }
}

/// A row: an integer, a CHAR(40), a DOUBLE PRECISION, and a last column
/// holding `long` bytes.
fn row(n: u64, long: usize) -> Row {
    vec![
        Value::Exact(Decimal::new(i128::from(n), 0)),
        Value::Char(format!("{:<40}", format!("name {n}"))),
        Value::Approx(n as f64),
        Value::Char(format!("{:<width$}", "x".repeat(long), width = last_length(long))),
    ]
}

/// A new, empty table of rows with a last column of `long` bytes, in a row
/// store of its own, in a new pager file and a pool that holds it.
fn table(bench: &Bench, long: usize) -> (RowStore, Arc<dyn TableStorage>) {
    let pool = harness::pool(FRAMES);
    let file = pool.add_pager(Arc::new(bench.pager("rows")));
    let store = RowStore::new(TableFiles::new(pool, file, Arc::new(MemoryLog::new())), TableOptions::default());
    let columns = [
        ("K", DataType::Exact { precision: 18, scale: 0 }),
        ("NAME", DataType::Character { length: 40 }),
        ("F", DataType::Approximate { precision: 53 }),
        ("LAST", DataType::Character { length: u32::try_from(last_length(long)).unwrap() }),
    ];
    let desc = TableDesc {
        name: QualifiedName { auth: "A".into(), ident: "T".into() },
        columns: columns
            .iter()
            .map(|&(name, data_type)| ColumnDesc { name: name.into(), data_type, not_null: false, default: None, domain: None })
            .collect(),
        constraints: vec![],
        engine: store.name().into(),
        options: Vec::new(),
        generation: 0,
        temporary: None,
    };
    let table = store.create_table(&desc).unwrap().into();
    (store, table)
}
