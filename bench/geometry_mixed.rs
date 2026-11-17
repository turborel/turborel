//! Sustained spatial reads and committed geometry moves. Each worker
//! owns a disjoint write partition; readers query the entire shared tree.

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
use std::time::Instant;

use harness::geometry::{ROWS, SELECT, database, id, position, window};
use harness::mixed::Samples;
use harness::{Bench, Case, on_threads};
use turborel::db::{Completion, Database};
use turborel::geometry::Geometry;
use turborel::value::{Decimal, Value};

const OPERATIONS: usize = 100_000;
const WRITE_EVERY: usize = 5;
const WIDTH: u64 = 10;
const NEAREST: &str = "SELECT ID, ST_DISTANCE(G, ?) AS D FROM POINTS ORDER BY D, ID FETCH FIRST 10 ROWS ONLY";
const FIRST: usize = 10;
const CHECKS: usize = 16;

#[derive(Clone, Copy)]
enum Read {
    Filter,
    Nearest,
}

impl Read {
    fn sql(self) -> &'static str {
        match self {
            Self::Filter => SELECT,
            Self::Nearest => NEAREST,
        }
    }

    fn argument(self, n: u64) -> Value {
        match self {
            Self::Filter => window(n, WIDTH),
            Self::Nearest => point(position(n)),
        }
    }
}

fn main() {
    let bench = Bench::new("geometry_mixed");
    let operations = std::env::var("TURBOREL_MIXED_OPERATIONS").map_or(OPERATIONS, |s| s.parse::<usize>().unwrap().max(WRITE_EVERY));
    let operations = operations / WRITE_EVERY * WRITE_EVERY;
    for (name, read) in [("width 10 filters", Read::Filter), ("nearest 10", Read::Nearest)] {
        for threads in [1_usize, 4, 8] {
            let case =
                Case::new(format!("{ROWS} points, 80% {name} / 20% updates, {operations} ops/worker"), "operations").threads(threads);
            bench.measure_mixed(case, database, |db| mixed(db, (threads, operations), read), |db| validate(db, threads, operations));
        }
    }
}

fn point((x, y): (u64, u64)) -> Value {
    Geometry::point(f64::from(u32::try_from(x).unwrap()), f64::from(u32::try_from(y).unwrap()), 0).unwrap().to_value()
}

fn moved(thread: usize, ordinal: usize, threads: usize) -> (u64, (u64, u64)) {
    let partition = usize::try_from(ROWS).unwrap() / threads;
    let id = ((ordinal % partition) * threads + thread) as u64;
    let cycle = ordinal / partition;
    (id, position(id + cycle as u64 + 1))
}

fn mixed(db: &Database, (threads, operations): (usize, usize), read: Read) -> Samples {
    let start = Barrier::new(threads);
    let samples = Mutex::new(Samples::default());
    on_threads(threads, |thread| {
        let a = db.session("B");
        let query = a.prepare(read.sql()).unwrap();
        let update = a.prepare("UPDATE POINTS SET G = ? WHERE ID = ?").unwrap();
        let mut own = Samples { reads: Vec::with_capacity(operations), writes: Vec::with_capacity(operations / WRITE_EVERY + 1) };
        start.wait();
        for n in 0..operations {
            if n % WRITE_EVERY == 0 {
                let (id, coordinate) = moved(thread, n / WRITE_EVERY, threads);
                let values = [point(coordinate), Value::Exact(Decimal::new(i128::from(id), 0))];
                let now = Instant::now();
                assert_eq!(a.execute_prepared(&update, &values).unwrap(), Completion::Success);
                a.commit().unwrap();
                own.writes.push(now.elapsed());
            } else {
                let value = read.argument((thread * operations + n) as u64);
                let now = Instant::now();
                let mut cursor = a.open_prepared(&query, &[value]).unwrap();
                let mut found = 0_usize;
                while let Some(row) = cursor.fetch().unwrap() {
                    black_box(row);
                    found += 1;
                }
                if matches!(read, Read::Nearest) {
                    assert_eq!(found, FIRST);
                }
                drop(cursor);
                a.commit().unwrap();
                own.reads.push(now.elapsed());
            }
        }
        samples.lock().unwrap().append(own);
    });
    samples.into_inner().unwrap()
}

fn final_positions(threads: usize, operations: usize) -> Vec<(u64, u64)> {
    let writes = operations / WRITE_EVERY;
    let partition = usize::try_from(ROWS).unwrap() / threads;
    // Count complete visits to each row independently of the worker's
    // write sequence. Rows not reached retain their original coordinates.
    (0..ROWS)
        .map(|id| {
            let ordinal = usize::try_from(id).unwrap() / threads;
            let visits = if ordinal < writes { (writes - 1 - ordinal) / partition + 1 } else { 0 };
            position(id + visits as u64)
        })
        .collect()
}

fn validate(db: &Database, threads: usize, operations: usize) {
    let expected = final_positions(threads, operations);
    let a = db.session("B");
    let mut cursor = a.open_cursor("SELECT ID, ST_X(G), ST_Y(G) FROM POINTS").unwrap();
    let mut visited = vec![false; usize::try_from(ROWS).unwrap()];
    while let Some(row) = cursor.fetch().unwrap() {
        let at = usize::try_from(id(&row[0])).unwrap();
        assert!(!visited[at]);
        visited[at] = true;
        let (x, y) = expected[at];
        assert_eq!(row[1].as_f64().unwrap().to_bits(), f64::from(u32::try_from(x).unwrap()).to_bits());
        assert_eq!(row[2].as_f64().unwrap().to_bits(), f64::from(u32::try_from(y).unwrap()).to_bits());
    }
    assert!(visited.into_iter().all(|found| found));
    drop(cursor);
    a.commit().unwrap();
    validate_indexes(db, &expected);
}

fn validate_indexes(db: &Database, points: &[(u64, u64)]) {
    let a = db.session("B");
    for read in [Read::Filter, Read::Nearest] {
        let query = a.prepare(read.sql()).unwrap();
        for n in 0..CHECKS {
            let n = n as u64 + ROWS / 2;
            let mut cursor = a.open_prepared(&query, &[read.argument(n)]).unwrap();
            let mut actual = Vec::new();
            while let Some(row) = cursor.fetch().unwrap() {
                actual.push(id(&row[0]));
            }
            let expected = match read {
                Read::Filter => {
                    actual.sort_unstable();
                    let (lx, ly) = position(n);
                    points
                        .iter()
                        .enumerate()
                        .filter_map(|(id, &(x, y))| (x >= lx && x <= lx + WIDTH && y >= ly && y <= ly + WIDTH).then_some(id as u64))
                        .collect()
                }
                Read::Nearest => nearest_ids(points, n),
            };
            assert_eq!(actual, expected);
        }
    }
    a.commit().unwrap();
}

fn nearest_ids(points: &[(u64, u64)], query: u64) -> Vec<u64> {
    let (x, y) = position(query);
    let mut ranked: Vec<_> = points
        .iter()
        .enumerate()
        .map(|(id, &(px, py))| {
            let dx = px.abs_diff(x);
            let dy = py.abs_diff(y);
            (dx * dx + dy * dy, id as u64)
        })
        .collect();
    ranked.select_nth_unstable(FIRST);
    ranked[..FIRST].sort_unstable();
    ranked[..FIRST].iter().map(|&(_, id)| id).collect()
}
