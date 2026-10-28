//! How fast a database kept in a directory changes rows (RULES.md D25,
//! D31): updates of every row of a table, committed in transactions of a
//! thousand rows, so what each changed row costs shows past each commit's
//! sync: its record in the log, and its older version in the undo log
//! (`undo/`); with and without a reader holding the older versions. And
//! how fast such a database recovers after a crash, in log records read; and
//! how fast every row's update, made in one transaction, is rolled back. Run
//! with `cargo xtask bench`, on the harness (RULES.md R-BENCH-1).

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

/// Rows of the table, and rows changed per transaction.
const ROWS: u64 = 10_000;
const PER_TRANSACTION: u64 = 1_000;

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
