//! How fast a database kept in a directory changes rows (RULES.md D25,
//! D31): updates of every row of a table, committed in transactions of a
//! thousand rows, so what each changed row costs shows past each commit's
//! sync: its record in the log, and its older version in the row store's
//! undo store (`engine/rows/undo.rs`); with and without a reader holding the older versions. And
//! how fast such a database recovers after a crash, in log records read; and
//! how fast every row's update, made in one transaction, is rolled back. Run
//! with `cargo xtask bench`, on the harness (RULES.md R-BENCH-1). And how
//! fast such a database scans a table ten times its buffer pool, of the
//! row store and of the column store, its groups frozen, with its pages
//! read ahead on its job (`turborel-read-ahead`, RULES.md D30) and with
//! none read ahead.

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
use std::path::PathBuf;
use std::sync::Arc;

use harness::{Bench, Case};
use turborel::db::{Database, OpenOptions, Opened};
use turborel::disk::io::{FileSystem, OsFileSystem};
use turborel::value::{Decimal, Value};

/// Rows of the table, and rows changed per transaction.
const ROWS: u64 = 10_000;
const PER_TRANSACTION: u64 = 1_000;
/// Rows of the tables scanned, each of some 200 bytes, and those each
/// INSERT of their setup writes.
const SCANNED: u64 = 100_000;
const PER_INSERT: u64 = 1_000;
/// The buffer pool of the database scanned: 2 MiB, a tenth of a table.
const SCAN_POOL: i128 = 2 << 20;

fn main() {
    let bench = Bench::new("kept");
    let made = Cell::new(0);
    bench.measure_fresh(Case::new("update rows, kept in a directory", "rows"), || database(&bench, &made), |db| updated(&db));
    bench.measure_fresh(
        Case::new("update rows, a reader holding older versions", "rows"),
        || database(&bench, &made),
        |db| {
            let reader = db.session("B");
            let mut cursor = reader.open_cursor("SELECT K FROM T").unwrap();
            cursor.fetch().unwrap();
            let rows = updated(&db);
            drop(cursor);
            rows
        },
    );
    bench.measure_fresh(
        Case::new("update every row in one transaction, then roll it back", "rows"),
        || database(&bench, &made),
        |db| {
            let s = db.session("B");
            s.run("UPDATE T SET V = V + 1").unwrap();
            s.rollback().unwrap();
            ROWS
        },
    );
    bench.measure_fresh(
        Case::new("recover after a crash, every row updated", "log records"),
        || {
            let (db, dir) = database_at(&bench, &made);
            updated(&db);
            // Not closed: opening it again recovers it.
            drop(db);
            dir
        },
        |dir| {
            let db = Database::new();
            let opened = db.open_files(&(Arc::new(OsFileSystem) as Arc<dyn FileSystem>), &dir, &OpenOptions::default()).unwrap();
            assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
            if let Opened::Recovered(report) = opened { report.records } else { 0 }
        },
    );
    scans(&bench);
}

/// Scans of tables ten times the pool, of the row store and the column
/// store, read ahead and not.
fn scans(bench: &Bench) {
    let db = Database::new();
    db.set_setting("STORAGE.POOL_BYTES", &Value::Exact(Decimal::new(SCAN_POOL, 0))).unwrap();
    db.open_files(&(Arc::new(OsFileSystem) as Arc<dyn FileSystem>), &bench.path("scanned"), &OpenOptions::default()).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION B CREATE TABLE R (K INTEGER, V CHAR(200))
           CREATE TABLE C (K INTEGER, V CHAR(200)) USING columnstore",
    )
    .unwrap();
    let s = db.session("B");
    for table in ["R", "C"] {
        for first in (0..SCANNED).step_by(PER_INSERT as usize) {
            let rows: Vec<String> = (first..first + PER_INSERT).map(|k| format!("({k}, 'value {k}')")).collect();
            s.run(&format!("INSERT INTO {table} VALUES {}", rows.join(", "))).unwrap();
        }
    }
    s.commit().unwrap();
    db.maintain().unwrap();
    for (table, store) in [("R", "row store"), ("C", "column store")] {
        for (pages, how) in [(0, "none read ahead"), (32, "read ahead on its job")] {
            db.set_setting("STORAGE.READ_AHEAD_PAGES", &Value::Exact(Decimal::new(pages, 0))).unwrap();
            bench.measure(Case::new(format!("scan a {store} table 10 times its pool, {how}"), "rows"), || {
                let mut cursor = s.open_cursor(&format!("SELECT K, V FROM {table}")).unwrap();
                let mut rows = 0;
                while cursor.fetch().unwrap().is_some() {
                    rows += 1;
                }
                s.commit().unwrap();
                rows
            });
        }
    }
}

/// Every row of T updated, in transactions of `PER_TRANSACTION` rows.
fn updated(db: &Database) -> u64 {
    let s = db.session("B");
    for from in (0..ROWS).step_by(PER_TRANSACTION as usize) {
        s.run(&format!("UPDATE T SET V = V + 1 WHERE K >= {from} AND K < {}", from + PER_TRANSACTION)).unwrap();
        s.commit().unwrap();
    }
    ROWS
}

/// A database in a new directory, schema B with T of `ROWS` rows.
fn database(bench: &Bench, made: &Cell<u32>) -> Database {
    database_at(bench, made).0
}

/// As `database`, with its directory.
fn database_at(bench: &Bench, made: &Cell<u32>) -> (Database, PathBuf) {
    made.set(made.get() + 1);
    let dir = bench.path(&format!("db{}", made.get()));
    let fs: Arc<dyn FileSystem> = Arc::new(OsFileSystem);
    let db = Database::new();
    db.open_files(&fs, &dir, &OpenOptions::default()).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V INTEGER, S CHAR(40))").unwrap();
    let s = db.session("B");
    for k in 0..ROWS {
        s.run(&format!("INSERT INTO T VALUES ({k}, 0, 'row {k}')")).unwrap();
    }
    s.commit().unwrap();
    (db, dir)
}
