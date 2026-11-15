//! Spatial workloads expressed with the same SQL for scans, B-trees and
//! R-trees: rectangular filters, points joined to rectangles, nearest
//! points by squared Euclidean distance, and readers sharing a table with
//! writers. The same statements remain the baseline when access paths
//! change. Every result is fetched; setup is outside the timed operation.
//! Run with `cargo xtask bench spatial --save before` (R-BENCH-1).

#![allow(
    clippy::unwrap_used,
    clippy::as_conversions,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::indexing_slicing,
    missing_docs,
    reason = "benchmark code: a failure ends the run (RULES.md R-DEV-3)"
)]

mod harness;

use std::hint::black_box;

use harness::{Bench, Case, on_threads, scatter};
use turborel::db::{Database, Session};
use turborel::txn::LockWait;

/// A SQL baseline small enough that its unindexed joins finish routinely.
const ROWS: u64 = 20_000;
/// Different permutations distribute coordinates across a square.
const SIDE: u64 = 1_000;
/// Queries per thread; both small windows and large scans use this count.
const QUERIES: u64 = 32;
/// Separate sessions, with the same total data at each thread count.
const THREADS: [usize; 3] = [1, 4, 8];
/// Rectangles on the smaller side of the spatial join.
const WINDOWS: u64 = 32;
/// One write for every five operations in the mixed workload.
const WRITE_EVERY: u64 = 5;
/// Operations of a mixed-workload session, including committed writes.
const MIXED_OPERATIONS: u64 = 100;
/// Points returned by a nearest-neighbour query; squared distance orders
/// these finite Cartesian coordinates exactly as Euclidean distance does.
const NEIGHBOURS: u64 = 10;

fn main() {
    let bench = Bench::new("spatial");
    for method in ["scan", "btree", "rtree"] {
        let db = database(method);
        for threads in THREADS {
            windows(&bench, &db, method, threads);
            nearest(&bench, &db, method, threads);
        }
        let session = db.session("B");
        bench.measure(Case::new(format!("point/rectangle join, {method}"), "joins"), || {
            fetch(
                &session,
                "SELECT P.ID, W.ID FROM POINTS P JOIN WINDOWS W
                 ON P.X >= W.LO_X AND P.X <= W.HI_X
                 AND P.Y >= W.LO_Y AND P.Y <= W.HI_Y",
            );
            1
        });
        for threads in THREADS {
            bench.measure_fresh(
                Case::new(format!("80% windows, 20% writes, {method}"), "operations").threads(threads),
                || database(method),
                |mixed| mix(&mixed, threads),
            );
        }
    }
}

fn windows(bench: &Bench, db: &Database, method: &str, threads: usize) {
    for width in [10_u64, 250] {
        bench.measure(Case::new(format!("window width {width}, {method}"), "queries").threads(threads), || {
            on_threads(threads, |thread| {
                let session = db.session("B");
                for n in 0..QUERIES {
                    fetch(&session, &window(thread as u64 * QUERIES + n, width));
                }
            });
            threads as u64 * QUERIES
        });
    }
}

fn nearest(bench: &Bench, db: &Database, method: &str, threads: usize) {
    bench.measure(Case::new(format!("nearest {NEIGHBOURS}, {method}"), "queries").threads(threads), || {
        on_threads(threads, |thread| {
            let session = db.session("B");
            for n in 0..QUERIES {
                let (x, y) = position(thread as u64 * QUERIES + n);
                let sql = format!(
                    "SELECT ID, (X - {x}) * (X - {x}) + (Y - {y}) * (Y - {y}) AS D
                     FROM POINTS ORDER BY D, ID FETCH FIRST {NEIGHBOURS} ROWS ONLY"
                );
                assert_eq!(fetch(&session, &sql), NEIGHBOURS);
            }
        });
        threads as u64 * QUERIES
    });
}

fn mix(db: &Database, threads: usize) -> u64 {
    on_threads(threads, |thread| {
        let session = db.session("B");
        for n in 0..MIXED_OPERATIONS {
            let own = thread as u64 * MIXED_OPERATIONS + n;
            if n % WRITE_EVERY == 0 {
                let (x, y) = position(own + ROWS + 1);
                session.run(&format!("UPDATE POINTS SET X = {x}, Y = {y} WHERE ID = {own}")).unwrap();
                session.commit().unwrap();
            } else {
                fetch(&session, &window(own, 10));
            }
        }
    });
    threads as u64 * MIXED_OPERATIONS
}

fn window(n: u64, width: u64) -> String {
    let (x, y) = position(n);
    format!("SELECT ID FROM POINTS WHERE X >= {x} AND X <= {} AND Y >= {y} AND Y <= {}", x + width, y + width)
}

fn position(n: u64) -> (u64, u64) {
    (scatter(n, ROWS) % SIDE, scatter(n * 37 + 17, ROWS) / (ROWS / SIDE))
}

/// The whole result reaches the caller, including rows beyond the first
/// cursor fetch, so the benchmark includes row access and materialization.
fn fetch(session: &Session, sql: &str) -> u64 {
    let mut cursor = session.open_cursor(sql).unwrap();
    let mut count = 0_u64;
    while let Some(row) = cursor.fetch().unwrap() {
        black_box(row);
        count += 1;
    }
    session.commit().unwrap();
    count
}

fn database(method: &str) -> Database {
    let db = Database::new();
    db.set_lock_wait(LockWait::Wait).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION B
         CREATE TABLE POINTS (ID INTEGER NOT NULL PRIMARY KEY, X DOUBLE PRECISION NOT NULL, Y DOUBLE PRECISION NOT NULL)
         CREATE TABLE WINDOWS (ID INTEGER NOT NULL, LO_X DOUBLE PRECISION NOT NULL, HI_X DOUBLE PRECISION NOT NULL,
                               LO_Y DOUBLE PRECISION NOT NULL, HI_Y DOUBLE PRECISION NOT NULL)",
    )
    .unwrap();
    let session = db.session("B");
    for id in 0..ROWS {
        let (x, y) = position(id);
        session.run(&format!("INSERT INTO POINTS VALUES ({id}, {x}, {y})")).unwrap();
    }
    for id in 0..WINDOWS {
        let (x, y) = position(id);
        session.run(&format!("INSERT INTO WINDOWS VALUES ({id}, {x}, {}, {y}, {})", x + 10, y + 10)).unwrap();
    }
    session.commit().unwrap();
    if method != "scan" {
        db.create_index("POINTS_XY", "B.POINTS", &["X", "Y"], method).unwrap();
    }
    db
}
