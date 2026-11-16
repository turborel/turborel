//! Complete nearest-geometry SQL with exact integer ranking oracles.
//! Candidates, refinement, sorting, fetching and commit are timed together.

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
use std::sync::{Barrier, Mutex};
use std::time::{Duration, Instant};

use harness::geometry::{ROWS, database, id, position};
use harness::{Bench, Case, on_threads};
use turborel::db::Database;
use turborel::geometry::Geometry;
use turborel::params::Params;
use turborel::value::{Decimal, Value};

const SELECT: &str = "SELECT ID, ST_DISTANCE(G, ?) AS D FROM POINTS ORDER BY D, ID FETCH FIRST 10 ROWS ONLY";
const QUERIES: usize = 16;
const FIRST: usize = 10;
const RECTANGLE_SIDE: u64 = 4;

fn main() {
    let bench = Bench::new("geometry_nearest");
    let queries = std::env::var("TURBOREL_NEAREST_QUERIES").map_or(QUERIES, |s| s.parse::<usize>().unwrap().max(1));
    for rectangles in [false, true] {
        let db = database();
        if rectangles {
            replace_points(&db);
        }
        validate(&db, rectangles, queries);
        let shape = if rectangles { "rectangles" } else { "points" };
        for threads in [1_usize, 4, 8] {
            let case = Case::new(format!("{ROWS} {shape}, nearest {FIRST}, {queries} queries/worker"), "queries").threads(threads);
            bench.measure_sampled(case, || queried(&db, threads, queries));
        }
        let a = db.session("B");
        let explain = SELECT.replace('?', "ST_POINT(5.25, 5.25)");
        harness::note(&format!("{shape} plan:\n{}", a.physical_plan(&explain, &Params::new()).unwrap()));
    }
}

fn replace_points(db: &Database) {
    let a = db.session("B");
    let update = a.prepare("UPDATE POINTS SET G = ? WHERE ID = ?").unwrap();
    for n in 0..ROWS {
        let (x, y) = position(n);
        let rectangle = Geometry::rectangle(
            [f64::from(u32::try_from(x).unwrap()), f64::from(u32::try_from(y).unwrap())],
            [f64::from(u32::try_from(x + RECTANGLE_SIDE).unwrap()), f64::from(u32::try_from(y + RECTANGLE_SIDE).unwrap())],
            0,
        )
        .unwrap()
        .to_value();
        a.execute_prepared(&update, &[rectangle, Value::Exact(Decimal::new(i128::from(n), 0))]).unwrap();
    }
    a.commit().unwrap();
}

fn query_point(n: usize) -> Value {
    let (x, y) = position(n as u64 + ROWS / 2);
    Geometry::point(f64::from(u32::try_from(x).unwrap()) + 0.25_f64, f64::from(u32::try_from(y).unwrap()) + 0.25_f64, 0).unwrap().to_value()
}

fn queried(db: &Database, threads: usize, queries: usize) -> Vec<Duration> {
    let start = Barrier::new(threads);
    let samples = Mutex::new(Vec::with_capacity(threads * queries));
    on_threads(threads, |thread| {
        let a = db.session("B");
        let query = a.prepare(SELECT).unwrap();
        let mut own = Vec::with_capacity(queries);
        start.wait();
        for n in 0..queries {
            let value = query_point(thread * queries + n);
            let now = Instant::now();
            let mut cursor = a.open_prepared(&query, &[value]).unwrap();
            let mut found = 0_usize;
            while let Some(row) = cursor.fetch().unwrap() {
                black_box(row);
                found += 1;
            }
            assert_eq!(found, FIRST);
            drop(cursor);
            a.commit().unwrap();
            own.push(now.elapsed());
        }
        samples.lock().unwrap().extend(own);
    });
    samples.into_inner().unwrap()
}

fn validate(db: &Database, rectangles: bool, queries: usize) {
    let a = db.session("B");
    let query = a.prepare(SELECT).unwrap();
    for n in 0..queries * 8 {
        let mut cursor = a.open_prepared(&query, &[query_point(n)]).unwrap();
        let mut actual = Vec::new();
        while let Some(row) = cursor.fetch().unwrap() {
            actual.push(id(&row[0]));
        }
        assert_eq!(actual, expected(n, rectangles), "query {n}, rectangles {rectangles}");
    }
    a.commit().unwrap();
    harness::note(&format!("validated {} nearest results against integer squared distances", queries * 8));
}

/// Coordinates scaled by four make each query offset integral. Rectangle
/// distance is the norm of its two outside gaps; exact squared distances
/// and IDs determine the same order without invoking any geometry code.
fn expected(query: usize, rectangles: bool) -> Vec<u64> {
    let (query_x, query_y) = position(u64::try_from(query).unwrap() + ROWS / 2);
    let (qx, qy) = (i64::try_from(query_x).unwrap() * 4 + 1, i64::try_from(query_y).unwrap() * 4 + 1);
    let width = if rectangles { i64::try_from(RECTANGLE_SIDE).unwrap() * 4 } else { 0 };
    let mut ranked: Vec<_> = (0..ROWS)
        .map(|n| {
            let (x, y) = position(n);
            let (x, y) = (i64::try_from(x).unwrap() * 4, i64::try_from(y).unwrap() * 4);
            let dx = (x - qx).max(qx - x - width).max(0);
            let dy = (y - qy).max(qy - y - width).max(0);
            (dx * dx + dy * dy, n)
        })
        .collect();
    ranked.select_nth_unstable(FIRST);
    ranked[..FIRST].sort_unstable();
    ranked[..FIRST].iter().map(|(_, n)| *n).collect()
}
