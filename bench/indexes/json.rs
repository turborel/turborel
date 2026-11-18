//! How fast a json index (`index/json.rs`) answers JSON_EXISTS beside a
//! scan of the same rows: documents of a few members, a filter of a value
//! that holds for one in a hundred, asked of a table with the index and of
//! one without, on one and several threads (RULES.md R-BENCH-1).

use std::hint::black_box;
use std::sync::{Barrier, Mutex};
use std::time::{Duration, Instant};

use super::harness::{self, Bench, Case, on_threads};
use turborel::db::Database;
use turborel::params::Params;

/// Rows of each table.
const ROWS: u64 = 20_000;
/// Queries of each thread in a run.
const QUERIES: usize = 8;
/// Thread counts.
const THREADS: [usize; 2] = [1, 4];
/// Distinct values of the member filtered, so one in a hundred holds.
const KINDS: u64 = 100;

/// Tables INDEXED, with the json index INDEXED_DOC, and PLAIN, of the same
/// documents.
fn database() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE INDEXED (ID INTEGER, DOC VARCHAR(300)) CREATE TABLE PLAIN (ID INTEGER, DOC VARCHAR(300))")
        .unwrap();
    let s = db.session("B");
    let docs: Vec<String> = (0..ROWS)
        .map(|n| format!(r#"({n}, '{{"kind":"k{}","n":{n},"tags":["a","b{}"],"owner":{{"id":{}}}}}')"#, n % KINDS, n % 7, n % 50))
        .collect();
    for chunk in docs.chunks(500) {
        for table in ["INDEXED", "PLAIN"] {
            s.run(&format!("INSERT INTO {table} VALUES {}", chunk.join(", "))).unwrap();
        }
    }
    s.commit().unwrap();
    s.run("CREATE INDEX INDEXED_DOC ON INDEXED (DOC) USING json").unwrap();
    db
}

/// The query of `table`, of the kind of its `n`th run.
fn query(table: &str, n: u64) -> String {
    format!("SELECT ID FROM {table} WHERE JSON_EXISTS(DOC, 'lax $ ? (@.kind == \"k{}\")')", n % KINDS)
}

/// The rows `sql` finds, each fetched.
fn count(s: &turborel::db::Session, sql: &str) -> usize {
    let mut cursor = s.open_cursor(sql).unwrap();
    let mut n = 0;
    while let Some(row) = cursor.fetch().unwrap() {
        black_box(row);
        n += 1;
    }
    n
}

/// Runs of `QUERIES` queries of `table` on each of `threads` threads.
fn queries(db: &Database, table: &str, threads: usize) -> Vec<Duration> {
    let start = Barrier::new(threads);
    let samples = Mutex::new(Vec::with_capacity(threads * QUERIES));
    on_threads(threads, |thread| {
        let s = db.session("B");
        let mut own = Vec::with_capacity(QUERIES);
        start.wait();
        for n in 0..QUERIES {
            let now = Instant::now();
            black_box(count(&s, &query(table, (thread * QUERIES + n) as u64)));
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
    let s = db.session("B");
    assert_eq!(count(&s, &query("INDEXED", 3)), count(&s, &query("PLAIN", 3)));
    s.commit().unwrap();
    for (table, how) in [("INDEXED", "json index probe"), ("PLAIN", "scan")] {
        for threads in THREADS {
            let case = Case::new(format!("JSON_EXISTS of 1 in {KINDS} of {ROWS} documents, {how}"), "queries").threads(threads);
            bench.measure_sampled(case, || queries(&db, table, threads));
        }
    }
    let plan = s.physical_plan(&query("INDEXED", 1), &Params::new()).unwrap();
    harness::note(&format!("probe plan:\n{plan}"));
}
