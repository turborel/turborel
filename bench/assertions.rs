//! How fast statements run that change a table an assertion reads: an
//! INSERT, an UPDATE and a DELETE of one row of a table of many, under an
//! assertion that no row breaks a rule, and under one that bounds the
//! table's rows; the same INSERT under an assertion of another form,
//! checked over the whole table; and under an assertion of a join, and of
//! a grouping, each beside the same assertion checked whole. Run with
//! `cargo xtask bench`, on the harness (RULES.md R-BENCH-1).

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
    asserted(&db, &s);
    let next = Cell::new(ROWS);
    for (table, under, statements) in INSERTED {
        bench.measure(Case::new(format!("insert one row, {under}"), "statements"), || {
            for _ in 0..statements {
                let k = next.get();
                next.set(k + 1);
                s.run(&format!("INSERT INTO {table} VALUES ({k}, 1)")).unwrap();
            }
            s.commit().unwrap();
            statements
        });
    }
    for (table, rule) in [("R", "a row rule"), ("C", "a bound on its rows")] {
        bench.measure(Case::new(format!("update one row, {rule}"), "statements"), || {
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

/// The tables each INSERT is of, the assertion it is under, and how many
/// make a run: fewer of a check reading its tables whole.
const INSERTED: [(&str, &str, u64); 7] = [
    ("R", "a row rule", STATEMENTS),
    ("C", "a bound on its rows", STATEMENTS),
    ("W", "a whole check", STATEMENTS),
    ("P", "a join", STATEMENTS),
    ("PW", "a join checked whole", STATEMENTS / 10),
    ("H", "a grouping", STATEMENTS),
    ("HW", "a grouping checked whole", STATEMENTS / 10),
];

/// Schema B's tables, each filled, and the assertions on them: of a join
/// of P and Q, and of a grouping of H, each beside the same checked whole,
/// of PW and QW, and of HW.
fn asserted(db: &Database, s: &Session) {
    let tables = ["R", "C", "W", "P", "Q", "PW", "QW", "H", "HW"];
    let made: Vec<String> = tables.iter().map(|t| format!("CREATE TABLE {t} (K INTEGER NOT NULL, N INTEGER)")).collect();
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION B {}", made.join(" "))).unwrap();
    for table in tables {
        fill(s, table);
    }
    s.run("CREATE ASSERTION NO_NEGATIVE CHECK (NOT EXISTS (SELECT * FROM R WHERE N < 0 OR N > K + 100000))").unwrap();
    s.run("CREATE ASSERTION BOUNDED CHECK ((SELECT COUNT(*) FROM C) <= 1000000)").unwrap();
    s.run("CREATE ASSERTION SUMMED CHECK ((SELECT SUM(N) FROM W) >= 0)").unwrap();
    for (name, whole) in [("", ""), ("W", " AND 1 = 1")] {
        let joined = format!("NOT EXISTS (SELECT * FROM P{name}, Q{name} WHERE P{name}.K = Q{name}.K AND P{name}.N > Q{name}.N){whole}");
        s.run(&format!("CREATE ASSERTION JOINED{name} CHECK ({joined})")).unwrap();
        let grouped = format!("NOT EXISTS (SELECT K FROM H{name} GROUP BY K HAVING COUNT(*) > 1000){whole}");
        s.run(&format!("CREATE ASSERTION GROUPED{name} CHECK ({grouped})")).unwrap();
    }
    s.commit().unwrap();
}

/// `table`, of `ROWS` rows, K 0 up, N 1, indexed by K.
fn fill(s: &Session, table: &str) {
    for k in 0..ROWS {
        s.run(&format!("INSERT INTO {table} VALUES ({k}, 1)")).unwrap();
    }
    s.commit().unwrap();
    s.run(&format!("CREATE INDEX {table}_K ON {table} (K)")).unwrap();
}
