//! How fast JSON_VALUE reads a member of a value of the JSON type, its
//! stored form read in place (RULES.md D89), beside the same
//! documents kept as JSON text, parsed at each call: a member near the
//! start and one near the end of documents of many members, on one and
//! several threads (RULES.md R-BENCH-1).

use std::hint::black_box;
use std::sync::{Barrier, Mutex};
use std::time::{Duration, Instant};

use super::harness::{Bench, Case, on_threads};
use turborel::db::Database;

/// Rows of each table.
const ROWS: u64 = 10_000;
/// Members of each document besides the two read.
const MEMBERS: u64 = 40;
/// Queries of each thread in a run.
const QUERIES: usize = 4;
/// Thread counts.
const THREADS: [usize; 2] = [1, 4];

/// Tables TYPED, of a JSON column, and TEXT, of a VARCHAR one, of the
/// same documents.
fn database() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE TYPED (ID INTEGER, DOC JSON) CREATE TABLE TEXT (ID INTEGER, DOC VARCHAR(4000))")
        .unwrap();
    let s = db.session("B");
    let docs: Vec<String> = (0..ROWS)
        .map(|n| {
            let middle: Vec<String> = (0..MEMBERS).map(|m| format!(r#""m{m}":{{"v":{m},"s":"text {m}"}}"#)).collect();
            format!(r#"({n}, '{{"first":{n},{},"last":"k{}"}}')"#, middle.join(","), n % 100)
        })
        .collect();
    for chunk in docs.chunks(200) {
        for table in ["TYPED", "TEXT"] {
            s.run(&format!("INSERT INTO {table} VALUES {}", chunk.join(", "))).unwrap();
        }
    }
    s.commit().unwrap();
    db
}

/// How many documents of `table` have the member `member`, each read.
fn query(table: &str, member: &str) -> String {
    format!("SELECT COUNT(JSON_VALUE(DOC, 'lax $.{member}')) FROM {table}")
}

/// Runs of `QUERIES` queries of `sql` on each of `threads` threads.
fn queries(db: &Database, sql: &str, threads: usize) -> Vec<Duration> {
    let start = Barrier::new(threads);
    let samples = Mutex::new(Vec::with_capacity(threads * QUERIES));
    on_threads(threads, |_| {
        let s = db.session("B");
        let mut own = Vec::with_capacity(QUERIES);
        start.wait();
        for _ in 0..QUERIES {
            let now = Instant::now();
            let mut cursor = s.open_cursor(sql).unwrap();
            while let Some(row) = cursor.fetch().unwrap() {
                black_box(row);
            }
            s.commit().unwrap();
            own.push(now.elapsed());
        }
        samples.lock().unwrap().extend(own);
    });
    samples.into_inner().unwrap()
}

/// The workloads, on each thread count.
pub fn workloads(bench: &Bench) {
    let db = database();
    for member in ["first", "last"] {
        for (table, how) in [("TYPED", "JSON type"), ("TEXT", "JSON text")] {
            for threads in THREADS {
                let case = Case::new(format!("JSON_VALUE of $.{member} of {ROWS} documents, {how}"), "queries").threads(threads);
                bench.measure_sampled(case, || queries(&db, &query(table, member), threads));
            }
        }
    }
}
