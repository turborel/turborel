//! SQL geometry filters and concurrent updates, with complete result
//! fetching and fresh databases for each mixed run (RULES.md R-BENCH-1).
//! Realistic parcels and roads, large joins, disk pressure and sustained
//! moves follow (`geometry_sql/realistic.rs`); `TURBOREL_GEOMETRY_REALISTIC`
//! runs them alone.

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
#[path = "geometry_sql/realistic.rs"]
mod realistic;
#[path = "geometry_sql/shapes.rs"]
mod shapes;

use std::hint::black_box;
use std::sync::{Barrier, Mutex};
use std::time::{Duration, Instant};

use harness::geometry::{ROWS, SELECT, database, position, validate_filters, window};
use harness::{Bench, Case, on_threads};
use turborel::db::Database;
use turborel::geometry::Geometry;
use turborel::params::Params;
use turborel::value::{Decimal, Value};

const THREADS: [usize; 3] = [1, 4, 8];
const QUERIES: usize = 16;
/// Queries per worker, `QUERIES` unless `TURBOREL_GEOMETRY_QUERIES` says otherwise, as
/// the nearest bench reads `TURBOREL_NEAREST_QUERIES`.
const QUERIES_VAR: &str = "TURBOREL_GEOMETRY_QUERIES";
const MIXED_OPERATIONS: u64 = 100;
const WRITE_EVERY: u64 = 5;

fn main() {
    let bench = Bench::new("geometry_sql");
    realistic::measure(&bench);
    if std::env::var("TURBOREL_GEOMETRY_REALISTIC").is_ok() {
        return;
    }
    let db = database();
    let count = std::env::var(QUERIES_VAR).map_or(QUERIES, |s| s.parse::<usize>().unwrap().max(1));
    let suffix = if count == QUERIES { String::new() } else { format!(", {count} queries/worker") };
    validate_filters(&db, (THREADS[2] * count) as u64);
    for width in [10_u64, 500] {
        for threads in THREADS {
            bench.measure_sampled(Case::new(format!("{ROWS} points, width {width}{suffix}"), "queries").threads(threads), || {
                queries(&db, threads, width, count)
            });
        }
    }
    for threads in THREADS {
        bench.measure_fresh(
            Case::new(format!("{ROWS} points, 80% filters / 20% updates"), "operations").threads(threads),
            database,
            |fresh| mixed(&fresh, threads),
        );
    }
    let a = db.session("B");
    let plan = a.physical_plan("SELECT ID FROM POINTS WHERE ST_INTERSECTS(G, ST_POINT(5, 5)) = 1", &Params::new()).unwrap();
    harness::note(&format!("selective plan:\n{plan}"));
}

fn queries(db: &Database, threads: usize, width: u64, count: usize) -> Vec<Duration> {
    let start = Barrier::new(threads);
    let samples = Mutex::new(Vec::with_capacity(threads * count));
    on_threads(threads, |thread| {
        let a = db.session("B");
        let query = a.prepare(SELECT).unwrap();
        let mut own = Vec::with_capacity(count);
        start.wait();
        for n in 0..count {
            let value = window((thread * count + n) as u64, width);
            let now = Instant::now();
            let mut cursor = a.open_prepared(&query, &[value]).unwrap();
            while let Some(row) = cursor.fetch().unwrap() {
                black_box(row);
            }
            drop(cursor);
            a.commit().unwrap();
            own.push(now.elapsed());
        }
        samples.lock().unwrap().extend(own);
    });
    samples.into_inner().unwrap()
}

fn mixed(db: &Database, threads: usize) -> u64 {
    let start = Barrier::new(threads);
    on_threads(threads, |thread| {
        let a = db.session("B");
        let query = a.prepare(SELECT).unwrap();
        let update = a.prepare("UPDATE POINTS SET G = ? WHERE ID = ?").unwrap();
        start.wait();
        for n in 0..MIXED_OPERATIONS {
            let id = thread as u64 * MIXED_OPERATIONS + n;
            if n % WRITE_EVERY == 0 {
                let (x, y) = position(id + ROWS + 1);
                let point =
                    Geometry::point(f64::from(u32::try_from(x).unwrap()), f64::from(u32::try_from(y).unwrap()), 0).unwrap().to_value();
                a.execute_prepared(&update, &[point, Value::Exact(Decimal::new(i128::from(id), 0))]).unwrap();
            } else {
                let mut cursor = a.open_prepared(&query, &[window(id, 10)]).unwrap();
                while let Some(row) = cursor.fetch().unwrap() {
                    black_box(row);
                }
            }
            a.commit().unwrap();
        }
    });
    threads as u64 * MIXED_OPERATIONS
}
