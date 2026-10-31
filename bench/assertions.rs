//! How fast statements run that change a table an assertion reads: an
//! INSERT, an UPDATE and a DELETE of one row of a table of many, under an
//! assertion that no row breaks a rule, and under one that bounds the
//! table's rows; and the same INSERT under an assertion of another form,
//! checked over the whole table. Run with `cargo xtask bench`, on the
//! harness (RULES.md R-BENCH-1).

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

use std::cell::Cell;

use harness::{Bench, Case};
use turborel::db::{Database, Session};

/// Rows in each table.
const ROWS: u64 = 20_000;
/// Statements in one run of a measurement.
const STATEMENTS: u64 = 500;

fn main() {
    let bench = Bench::new("assertions");
    let db = Database::new();
    let s = db.session("B");
    db.schema(
        "CREATE SCHEMA AUTHORIZATION B \
         CREATE TABLE R (K INTEGER NOT NULL, N INTEGER) \
         CREATE TABLE C (K INTEGER NOT NULL, N INTEGER) \
         CREATE TABLE W (K INTEGER NOT NULL, N INTEGER)",
    )
    .unwrap();
    for table in ["R", "C", "W"] {
        fill(&s, table);
    }
    s.run("CREATE ASSERTION NO_NEGATIVE CHECK (NOT EXISTS (SELECT * FROM R WHERE N < 0 OR N > K + 100000))").unwrap();
    s.run("CREATE ASSERTION BOUNDED CHECK ((SELECT COUNT(*) FROM C) <= 1000000)").unwrap();
    s.run("CREATE ASSERTION SUMMED CHECK ((SELECT SUM(N) FROM W) >= 0)").unwrap();
    s.commit().unwrap();
    let next = Cell::new(ROWS);
    for (table, under) in [("R", "a row rule"), ("C", "a bound on its rows"), ("W", "a whole check")] {
        bench.measure(Case::new(format!("insert one row, {under}"), "statements"), || {
            for _ in 0..STATEMENTS {
                let k = next.get();
                next.set(k + 1);
                s.run(&format!("INSERT INTO {table} VALUES ({k}, 1)")).unwrap();
            }
            s.commit().unwrap();
            STATEMENTS
        });
    }
    for (table, under) in [("R", "a row rule"), ("C", "a bound on its rows")] {
        bench.measure(Case::new(format!("update one row, {under}"), "statements"), || {
            for i in 0..STATEMENTS {
                s.run(&format!("UPDATE {table} SET N = N + 1 WHERE K = {}", harness::scatter(i, ROWS))).unwrap();
            }
            s.commit().unwrap();
            STATEMENTS
        });
    }
    let gone = Cell::new(0_u64);
    bench.measure(Case::new("delete one row, a bound on its rows", "statements"), || {
        for _ in 0..STATEMENTS {
            let k = gone.get();
            gone.set(k + 1);
            s.run(&format!("DELETE FROM C WHERE K = {k}")).unwrap();
        }
        s.commit().unwrap();
        STATEMENTS
    });
}

/// `table`, of `ROWS` rows, K 0 up, N 1, indexed by K.
fn fill(s: &Session, table: &str) {
    for k in 0..ROWS {
        s.run(&format!("INSERT INTO {table} VALUES ({k}, 1)")).unwrap();
    }
    s.commit().unwrap();
    s.run(&format!("CREATE INDEX {table}_K ON {table} (K)")).unwrap();
}
