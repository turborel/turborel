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
//! none read ahead. And, of a table of 16 columns many times its pool, of
//! the row store, of the column store and of it laid out as PAX, how fast
//! a row is fetched by its key, whole and two of its columns, and two
//! columns and all are scanned: what each layout reads of a row (RULES.md
//! D31). And, of such a table of the row store, held by its pool and many
//! times it, what the planner's weights price (RULES.md D35): a row
//! scanned, a row fetched by an index of another column, and the entries
//! of that index alone; a join of keys by the key's index and by a hash
//! join.

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

use harness::{Bench, Case, scatter};
use turborel::db::{Database, OpenOptions, Opened, Session};
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
/// A buffer pool that holds a table of 16 columns whole: 128 MiB.
const HOLDING_POOL: i128 = 128 << 20;
/// Rows of the tables of 16 columns, and the rows fetched by key a run.
const WIDE: u64 = 200_000;
const FETCHED: u64 = 2_000;
/// Keys a join of the weights' finds by the table's key: where, in a pool
/// many times smaller than the table, its index and a hash join cost
/// about the same.
const JOINED: u64 = 3_000;

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
    wide(&bench);
    for (pool, held) in [(HOLDING_POOL, "held by its pool"), (SCAN_POOL, "many times its pool")] {
        weights(&bench, pool, held);
    }
}

/// Of tables of 16 columns of scattered numbers, WR of the row store, WC
/// of the column store and WP of it laid out as PAX (COLUMNSTORE.LAYOUT),
/// their groups frozen, many times the pool: rows
/// fetched by key, whole and two columns of each, and two columns and all
/// scanned.
fn wide(bench: &Bench) {
    let db = Database::new();
    db.set_setting("STORAGE.POOL_BYTES", &Value::Exact(Decimal::new(SCAN_POOL, 0))).unwrap();
    db.open_files(&(Arc::new(OsFileSystem) as Arc<dyn FileSystem>), &bench.path("wide"), &OpenOptions::default()).unwrap();
    let columns: Vec<String> = (1_u32..16).map(|c| format!("C{c} INTEGER")).collect();
    db.schema(&format!(
        "CREATE SCHEMA AUTHORIZATION B CREATE TABLE WR (K INTEGER PRIMARY KEY, {0}) CREATE TABLE WC (K INTEGER PRIMARY KEY, {0}) USING columnstore
           CREATE TABLE WP (K INTEGER PRIMARY KEY, {0}) USING columnstore WITH (COLUMNSTORE.LAYOUT = 'PAX')",
        columns.join(", ")
    ))
    .unwrap();
    let s = db.session("B");
    for table in ["WR", "WC", "WP"] {
        filled(&s, table);
    }
    s.commit().unwrap();
    db.maintain().unwrap();
    let s = &s;
    let run = |sql: String| move || fetched(s, &sql);
    for (table, store) in [("WR", "row store"), ("WC", "column store"), ("WP", "column store laid out as PAX")] {
        for (what, list) in [("whole", "*"), ("two of its columns", "C3, C11")] {
            bench.measure(Case::new(format!("fetch a row of 16 columns by key, {what}, {store}"), "queries"), || {
                for n in 0..FETCHED {
                    run(format!("SELECT {list} FROM {table} WHERE K = {}", scatter(n, WIDE)))();
                }
                FETCHED
            });
        }
        bench.measure(Case::new(format!("scan 2 of 16 columns, {store}"), "rows"), run(format!("SELECT C3, C11 FROM {table}")));
        bench.measure(Case::new(format!("scan all 16 columns, {store}"), "rows"), run(format!("SELECT * FROM {table}")));
    }
}

/// Writes `WIDE` rows of 16 columns of scattered numbers to `table`, its
/// key the first.
fn filled(s: &Session, table: &str) {
    for first in (0..WIDE).step_by(PER_INSERT as usize) {
        let rows: Vec<String> = (first..first + PER_INSERT)
            .map(|k| format!("({k}, {})", (1..16).map(|c| scatter(k * 16 + c, WIDE * 16).to_string()).collect::<Vec<_>>().join(", ")))
            .collect();
        s.run(&format!("INSERT INTO {table} VALUES {}", rows.join(", "))).unwrap();
    }
}

/// The rows of `sql`, each fetched, counted.
fn fetched(s: &Session, sql: &str) -> u64 {
    let mut cursor = s.open_cursor(sql).unwrap();
    let mut rows = 0;
    while cursor.fetch().unwrap().is_some() {
        rows += 1;
    }
    s.commit().unwrap();
    rows
}

/// What the planner's weights price, of WR, a table of 16 columns of the
/// row store, in a database of a buffer pool of `pool` bytes, `held` as
/// it says: a column of every row summed; a column of 1,000 rows summed,
/// fetched by an index of another column, and the entries alone of that
/// index counted; and a column summed of `JOINED` scattered keys joined
/// to the rows of those keys by the key's index and, the index's descents
/// priced past any scan (PLANNER.INDEX_DESCENT_COST), by a hash join.
fn weights(bench: &Bench, pool: i128, held: &str) {
    let db = Database::new();
    db.set_setting("STORAGE.POOL_BYTES", &Value::Exact(Decimal::new(pool, 0))).unwrap();
    db.open_files(&(Arc::new(OsFileSystem) as Arc<dyn FileSystem>), &bench.path(&format!("weights-{pool}")), &OpenOptions::default())
        .unwrap();
    let columns: Vec<String> = (1_u32..16).map(|c| format!("C{c} INTEGER")).collect();
    db.schema(&format!(
        "CREATE SCHEMA AUTHORIZATION B CREATE TABLE WR (K INTEGER PRIMARY KEY, {}) CREATE TABLE KS (K INTEGER)",
        columns.join(", ")
    ))
    .unwrap();
    db.create_index("WR_C1", "B.WR", &["C1"], "btree").unwrap();
    let s = db.session("B");
    filled(&s, "WR");
    let keys: Vec<String> = (0..JOINED).map(|n| format!("({})", scatter(n, WIDE))).collect();
    s.run(&format!("INSERT INTO KS VALUES {}", keys.join(", "))).unwrap();
    s.commit().unwrap();
    db.maintain().unwrap();
    // C1 is of WIDE * 16 values scattered, so 16,000 of them hold 1,000 rows.
    for (what, sql) in [
        ("scan 1 of 16 columns", "SELECT SUM(C3) FROM WR"),
        ("fetch 1,000 rows by an index of another column", "SELECT SUM(C3) FROM WR WHERE C1 BETWEEN 0 AND 15999"),
        ("count 1,000 entries of that index alone", "SELECT COUNT(*) FROM WR WHERE C1 BETWEEN 0 AND 15999"),
    ] {
        bench.measure(Case::new(format!("{what}, {held}"), "queries"), || fetched(&s, sql));
    }
    let join = "SELECT SUM(WR.C3) FROM KS, WR WHERE WR.K = KS.K";
    for (how, descent) in [("its index", turborel::physical::DEFAULT_INDEX_DESCENT), ("a hash join", 1.0e12_f64)] {
        db.set_setting("PLANNER.INDEX_DESCENT_COST", &Value::Approx(descent)).unwrap();
        bench.measure(Case::new(format!("join {JOINED} keys by {how}, {held}"), "queries"), || fetched(&s, join));
    }
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
