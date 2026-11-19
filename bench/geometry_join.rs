//! Complete SQL geometry joins, validated against integer-coordinate
//! inclusion before timing (RULES.md R-BENCH-1).

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
#[path = "geometry_join/large.rs"]
mod large;

use std::hint::black_box;
use std::sync::Barrier;

use harness::geometry::{ROWS, database, id, ids, window};
use harness::{Bench, Case, on_threads};
use turborel::db::Database;
use turborel::params::Params;
use turborel::value::{Decimal, Value};

const WINDOWS: u64 = 32;
const JOIN: &str = "SELECT P.ID, W.ID FROM WINDOWS W JOIN POINTS P ON ST_INTERSECTS(P.G, W.G) = 1";
const REVERSED: &str = "SELECT P.ID, W.ID FROM POINTS P JOIN WINDOWS W ON ST_INTERSECTS(P.G, W.G) = 1";

fn main() {
    let bench = Bench::new("geometry_join");
    for width in [10_u64, 500] {
        let db = database();
        windows(&db, width);
        for (query, order) in [(JOIN, ""), (REVERSED, ", points first")] {
            validate(&db, width, query);
            for threads in [1_usize, 4, 8] {
                let case = Case::new(format!("{ROWS} points, {WINDOWS} windows, width {width}{order}"), "joins").threads(threads);
                bench.measure(case, || joined(&db, threads, query));
            }
            let a = db.session("B");
            harness::note(&format!("join plan (width {width}{order}):\n{}", a.physical_plan(query, &Params::new()).unwrap()));
        }
    }
    large::measure(&bench, &database());
}

fn windows(db: &Database, width: u64) {
    let a = db.session("B");
    a.run("CREATE TABLE WINDOWS (ID INTEGER, G GEOMETRY(0))").unwrap();
    let insert = a.prepare("INSERT INTO WINDOWS VALUES (?, ?)").unwrap();
    for n in 0..WINDOWS {
        a.execute_prepared(&insert, &[Value::Exact(Decimal::new(i128::from(n), 0)), window(n, width)]).unwrap();
    }
    a.commit().unwrap();
}

fn joined(db: &Database, threads: usize, query: &str) -> u64 {
    let start = Barrier::new(threads);
    on_threads(threads, |_| {
        let a = db.session("B");
        start.wait();
        let mut cursor = a.open_cursor(query).unwrap();
        while let Some(row) = cursor.fetch().unwrap() {
            black_box(row);
        }
        drop(cursor);
        a.commit().unwrap();
    });
    threads as u64
}

fn validate(db: &Database, width: u64, query: &str) {
    let a = db.session("B");
    let mut cursor = a.open_cursor(query).unwrap();
    let mut actual = Vec::new();
    while let Some(row) = cursor.fetch().unwrap() {
        actual.push((id(&row[0]), id(&row[1])));
    }
    drop(cursor);
    a.commit().unwrap();
    let mut expected: Vec<_> = (0..WINDOWS).flat_map(|n| ids(n, width).into_iter().map(move |point| (point, n))).collect();
    actual.sort_unstable();
    expected.sort_unstable();
    assert_eq!(actual, expected, "join width {width}");
    harness::note(&format!("join width {width}: {} validated pairs", actual.len()));
}
