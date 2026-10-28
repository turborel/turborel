//! How fast cursors give rows: opening a cursor over a
//! table and fetching its first row, and fetching every row, through the
//! Rust API and through OPEN and FETCH statements. Run with `cargo xtask
//! bench`, on the harness (RULES.md R-BENCH-1).

#![allow(
    clippy::unwrap_used,
    clippy::as_conversions,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    missing_docs,
    reason = "benchmark code: a failure ends the run (RULES.md R-DEV-3)"
)]

mod harness;

use std::sync::Arc;

use harness::{Bench, Case};
use turborel::db::{Completion, Database};
use turborel::engine::rows::{ROW_STORE, RowStore, TableFiles, TableOptions};
use turborel::log::MemoryLog;
use turborel::params::Params;
use turborel::value::{DataType, Value};

/// Rows in the table, and frames: enough for the whole table.
const ROWS: u64 = 200_000;
const FRAMES: usize = 32_768;

fn main() {
    let bench = Bench::new("cursors");
    let db = database(&bench);
    let s = db.session("B");
    bench.measure(Case::new("open, fetch the first row", "cursors"), || {
        let mut cursor = s.open_cursor("SELECT K, NAME FROM T").unwrap();
        cursor.fetch().unwrap().unwrap();
        s.commit().unwrap();
        1
    });
    bench.measure(Case::new("open, fetch every row", "rows"), || {
        let mut cursor = s.open_cursor("SELECT K, NAME FROM T").unwrap();
        let mut rows = 0;
        while cursor.fetch().unwrap().is_some() {
            rows += 1;
        }
        s.commit().unwrap();
        rows
    });
    s.declare_cursor("DECLARE C CURSOR FOR SELECT K FROM T").unwrap();
    let mut p = Params::new().declare("OUT", DataType::Exact { precision: 9, scale: 0 }, Value::Null);
    bench.measure(Case::new("OPEN, FETCH every row", "rows"), || {
        s.execute("OPEN C", &mut p).unwrap();
        let mut rows = 0;
        while s.execute("FETCH C INTO OUT", &mut p).unwrap() == Completion::Success {
            rows += 1;
        }
        s.execute("CLOSE C", &mut p).unwrap();
        s.commit().unwrap();
        rows
    });
}

/// Schema B with table T of `ROWS` rows, in a row store in the
/// benchmark's directory.
fn database(bench: &Bench) -> Database {
    let pool = harness::pool(FRAMES);
    let file = pool.add_pager(Arc::new(bench.pager("rows")));
    let db = Database::new();
    db.register_engine(Box::new(RowStore::new(TableFiles::new(pool, file, Arc::new(MemoryLog::new())), TableOptions::default()))).unwrap();
    db.use_engine(ROW_STORE).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE T (K INTEGER NOT NULL, NAME CHAR(20))").unwrap();
    let s = db.session("B");
    for k in 0..ROWS {
        s.run(&format!("INSERT INTO T VALUES ({k}, 'name {k}')")).unwrap();
    }
    s.commit().unwrap();
    db
}
