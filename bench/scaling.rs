//! How throughput scales with threads (RULES.md D29), on the
//! row store (D31): each workload runs at several thread counts, on a
//! database of its own for each run, and reports transactions per second
//! and the speedup over one thread. Run with `cargo xtask bench`, on the
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
use std::sync::Arc;

use harness::{Bench, Case, crc, on_threads};
use turborel::db::{Database, Session};
use turborel::engine::rows::{ROW_STORE, RowStore, TableFiles, TableOptions};
use turborel::error::Code;
use turborel::index::BTreeMethod;
use turborel::log::{FileLog, FileLogOptions, MemoryLog};
use turborel::txn::LockWait;

/// The thread counts each workload runs at; the most, so every workload
/// can give each thread a table of its own.
const THREAD_COUNTS: [usize; 4] = [1, 2, 4, 8];
const MOST_THREADS: usize = 8;
/// Rows in the table the readers count.
const READ_TABLE_ROWS: usize = 1_000;
/// Transactions each thread of a writing workload runs.
const WRITES_PER_THREAD: usize = 20_000;
/// Rows of the table each thread updates its own rows of, per thread.
const OWN_ROWS: usize = 1_000;
/// Frames in each database's buffer pool.
const FRAMES: usize = 16_384;

/// One workload: how to build its database, how many transactions each
/// thread runs, and one transaction of thread `thread` numbered `n`.
struct Workload {
    name: &'static str,
    setup: fn(&Bench, &Cell<usize>) -> Database,
    per_thread: usize,
    transaction: fn(&Session, usize, usize),
}

const WORKLOADS: [Workload; 6] = [
    Workload { name: "read one table", setup: read_setup, per_thread: 2_000, transaction: read },
    Workload { name: "write separate tables", setup: tables_setup, per_thread: WRITES_PER_THREAD, transaction: write_own_table },
    Workload { name: "write one table", setup: tables_setup, per_thread: WRITES_PER_THREAD, transaction: write_shared_table },
    Workload { name: "write one keyed table", setup: tables_setup, per_thread: WRITES_PER_THREAD, transaction: write_keyed_table },
    Workload { name: "update own rows", setup: update_setup, per_thread: 5_000, transaction: update_own_rows },
    Workload { name: "write, file log", setup: logged_setup, per_thread: 100, transaction: write_own_table },
];

fn main() {
    let bench = Bench::new("scaling");
    // Numbers each database's directory.
    let made = Cell::new(0);
    for workload in &WORKLOADS {
        for threads in THREAD_COUNTS {
            bench.measure_fresh(
                Case::new(workload.name, "transactions").threads(threads),
                || (workload.setup)(&bench, &made),
                |db| run(&db, workload, threads),
            );
        }
    }
}

/// Runs `workload` on `threads` threads, each on a session of its own;
/// the transactions run.
fn run(db: &Database, workload: &Workload, threads: usize) -> u64 {
    on_threads(threads, |t| {
        let s = db.session("B");
        for n in 0..workload.per_thread {
            (workload.transaction)(&s, t, n);
        }
    });
    (threads * workload.per_thread) as u64
}

/// A new database whose tables are in a row store, in a new directory,
/// older versions in memory, indexes in a file of their own in the same
/// pool.
fn rows_database(bench: &Bench, made: &Cell<usize>) -> Database {
    made.set(made.get() + 1);
    let dir = bench.path(&made.get().to_string());
    std::fs::create_dir_all(&dir).unwrap();
    let pool = harness::pool(FRAMES);
    let file = pool.add_pager(Arc::new(bench.pager(&format!("{}/rows", made.get()))));
    let indexes = pool.add_pager(Arc::new(bench.pager(&format!("{}/indexes", made.get()))));
    let db = Database::new();
    db.register_index_method(Box::new(BTreeMethod::new(Arc::clone(&pool), indexes))).unwrap();
    db.register_engine(Box::new(RowStore::new(TableFiles::new(pool, file, Arc::new(MemoryLog::new())), TableOptions::default()))).unwrap();
    db.use_engine(ROW_STORE).unwrap();
    db
}

/// Schema B with table R of `READ_TABLE_ROWS` rows.
fn read_setup(bench: &Bench, made: &Cell<usize>) -> Database {
    let db = rows_database(bench, made);
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE R (K INTEGER NOT NULL)").unwrap();
    let s = db.session("B");
    for k in 0..READ_TABLE_ROWS {
        s.run(&format!("INSERT INTO R VALUES ({k})")).unwrap();
    }
    s.commit().unwrap();
    db
}

/// Counts R's rows and commits.
fn read(s: &Session, _thread: usize, _n: usize) {
    let mut cursor = s.open_cursor("SELECT COUNT(*) FROM R").unwrap();
    cursor.fetch().unwrap();
    s.commit().unwrap();
}

/// Schema B with tables T0 to T7, S, and I, keyed, writers waiting for
/// locks.
fn tables_setup(bench: &Bench, made: &Cell<usize>) -> Database {
    let db = rows_database(bench, made);
    let tables: Vec<String> = (0..MOST_THREADS).map(|t| format!("CREATE TABLE T{t} (K INTEGER NOT NULL)")).collect();
    let shared = "CREATE TABLE S (K INTEGER NOT NULL) CREATE TABLE I (K INTEGER NOT NULL UNIQUE)";
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION B {} {shared}", tables.join(" "))).unwrap();
    db.set_lock_wait(LockWait::Wait).unwrap();
    db
}

/// Inserts a row into the thread's own table and commits.
fn write_own_table(s: &Session, thread: usize, n: usize) {
    s.run(&format!("INSERT INTO T{thread} VALUES ({n})")).unwrap();
    s.commit().unwrap();
}

/// Inserts a row into S, which every thread writes, and commits.
fn write_shared_table(s: &Session, thread: usize, n: usize) {
    s.run(&format!("INSERT INTO S VALUES ({})", thread * WRITES_PER_THREAD + n)).unwrap();
    s.commit().unwrap();
}

/// Inserts a row into I, which every thread writes, its key in the index
/// of its UNIQUE constraint, and commits.
fn write_keyed_table(s: &Session, thread: usize, n: usize) {
    s.run(&format!("INSERT INTO I VALUES ({})", thread * WRITES_PER_THREAD + n)).unwrap();
    s.commit().unwrap();
}

/// Schema B with table U of `OWN_ROWS` rows for each thread, keyed, writers
/// waiting for locks.
fn update_setup(bench: &Bench, made: &Cell<usize>) -> Database {
    let db = rows_database(bench, made);
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE U (K INTEGER NOT NULL UNIQUE, V INTEGER)").unwrap();
    db.set_lock_wait(LockWait::Wait).unwrap();
    let s = db.session("B");
    for k in 0..MOST_THREADS * OWN_ROWS {
        s.run(&format!("INSERT INTO U VALUES ({k}, 0)")).unwrap();
    }
    s.commit().unwrap();
    db
}

/// Adds 1 to one of the thread's own rows of U, by key, and commits: no
/// two threads write one row. A transaction refused (SQLCODE -911) is run
/// again, as a client would.
fn update_own_rows(s: &Session, thread: usize, n: usize) {
    let sql = format!("UPDATE U SET V = V + 1 WHERE K = {}", thread * OWN_ROWS + n % OWN_ROWS);
    loop {
        let Err(e) = s.run(&sql).and_then(|_| s.commit()) else { return };
        assert_eq!(e.code, Code::Conflict, "{e:?}");
        s.rollback().unwrap();
    }
}

/// `tables_setup`, logging to a new file log: every commit syncs its
/// record, grouped with those of other threads.
fn logged_setup(bench: &Bench, made: &Cell<usize>) -> Database {
    let db = tables_setup(bench, made);
    let log = FileLog::create(
        Arc::new(turborel::disk::io::OsFileSystem),
        &bench.path(&format!("{}/log", made.get())),
        crc(),
        FileLogOptions::default(),
    )
    .unwrap();
    db.set_log(Arc::new(log)).unwrap();
    db
}
