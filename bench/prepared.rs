//! How fast prepared statements run: a query of one row by an indexed
//! key, an UPDATE of one row by it, and an INSERT of one row, each
//! prepared once and executed many times with new values; beside the
//! same query as direct SQL text, planned anew each time. Run with
//! `cargo xtask bench`, on the harness (RULES.md R-BENCH-1).

#![allow(
    clippy::unwrap_used,
    clippy::as_conversions,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    missing_docs,
    reason = "benchmark code: a failure ends the run (RULES.md R-DEV-3)"
)]

mod harness;

use std::cell::Cell;

use harness::{Bench, Case};
use turborel::db::Database;
use turborel::value::{Decimal, Value};

/// Rows in the table.
const ROWS: u64 = 100_000;
/// Executions in one run of a measurement.
const EXECUTIONS: u64 = 20_000;

fn main() {
    let bench = Bench::new("prepared");
    let db = database();
    let s = db.session("B");
    let query = s.prepare("SELECT NAME FROM T WHERE K = ?").unwrap();
    bench.measure(Case::new("query one row by its key", "executions"), || {
        for i in 0..EXECUTIONS {
            let mut cursor = s.open_prepared(&query, &[int(harness::scatter(i, ROWS))]).unwrap();
            cursor.fetch().unwrap().unwrap();
        }
        s.commit().unwrap();
        EXECUTIONS
    });
    bench.measure(Case::new("query one row by its key, direct", "executions"), || {
        for i in 0..EXECUTIONS {
            let k = harness::scatter(i, ROWS);
            let mut cursor = s.open_cursor(&format!("SELECT NAME FROM T WHERE K = {k}")).unwrap();
            cursor.fetch().unwrap().unwrap();
        }
        s.commit().unwrap();
        EXECUTIONS
    });
    let update = s.prepare("UPDATE T SET N = N + 1 WHERE K = ?").unwrap();
    bench.measure(Case::new("update one row by its key", "executions"), || {
        for i in 0..EXECUTIONS {
            s.execute_prepared(&update, &[int(harness::scatter(i, ROWS))]).unwrap();
        }
        s.commit().unwrap();
        EXECUTIONS
    });
    let insert = s.prepare("INSERT INTO U VALUES (?, ?)").unwrap();
    let next = Cell::new(0_u64);
    bench.measure(Case::new("insert one row", "executions"), || {
        for _ in 0..EXECUTIONS {
            let k = next.get();
            next.set(k + 1);
            s.execute_prepared(&insert, &[int(k), Value::Char(format!("name {k}"))]).unwrap();
        }
        s.commit().unwrap();
        EXECUTIONS
    });
}

/// `k` as an INTEGER's value.
fn int(k: u64) -> Value {
    Value::Exact(Decimal::new(i128::from(k), 0))
}

/// Schema B with table T of `ROWS` rows, indexed by its key, and an
/// empty table U, in memory.
fn database() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION B \
         CREATE TABLE T (K INTEGER NOT NULL, NAME CHAR(20), N INTEGER) \
         CREATE TABLE U (K INTEGER NOT NULL, NAME CHAR(20))",
    )
    .unwrap();
    let s = db.session("B");
    for k in 0..ROWS {
        s.run(&format!("INSERT INTO T VALUES ({k}, 'name {k}', 0)")).unwrap();
    }
    s.commit().unwrap();
    db.create_index("T_K", "B.T", &["K"], "btree").unwrap();
    db
}
