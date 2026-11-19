//! Complete queries over realistic shapes (`shapes.rs`), with p50 and p99
//! latency: a join of large inputs on both sides, parcels against roads;
//! window filters over parcels of many vertices and distance filters over
//! roads; the same filters in a database on disk whose buffer pool is a
//! small share of its data; and filters while writers move parcels.

use std::hint::black_box;
use std::sync::{Arc, Barrier, Mutex};
use std::time::{Duration, Instant};

use super::harness::mixed::Samples;
use super::harness::{Bench, Case, on_threads};
use super::shapes;
use turborel::db::{Completion, Database, OpenOptions, Prepared, Session};
use turborel::disk::io::{FileSystem, OsFileSystem};
use turborel::geometry::Geometry;
use turborel::params::Params;
use turborel::txn::LockWait;
use turborel::value::{Decimal, Value};

const JOIN: &str = "SELECT P.ID, R.ID FROM ROADS R JOIN PARCELS P ON ST_INTERSECTS(P.G, R.G) = 1";
const WINDOW: &str = "SELECT ID, G FROM PARCELS WHERE ST_INTERSECTS(G, ?) = 1";
const NEAR: &str = "SELECT ID FROM ROADS WHERE ST_DISTANCE(G, ?) <= 150";
/// Queries a filter run makes, the windows' side, and a small pool.
const QUERIES: u64 = 64;
const WIDTH: f64 = 400.0;
const SMALL_POOL: i128 = 8 << 20;
/// Operations of each worker of the sustained case, one in five a move.
const OPERATIONS: usize = 2_000;
const WRITE_EVERY: usize = 5;

/// Runs every realistic workload once, on the harness.
pub fn measure(bench: &Bench) {
    let (parcels, roads) = (shapes::parcels(), shapes::roads());
    let db = loaded(None, (&parcels, &roads));
    plans(&db);
    bench.measure_sampled(Case::new(format!("{} parcels x {} roads join", parcels.len(), roads.len()), "queries"), || {
        vec![timed(|| {
            black_box(complete(&db, JOIN, &[]));
        })]
    });
    filters(bench, &db, "in memory");
    drop(db);
    let disk = loaded(Some(bench), (&parcels, &roads));
    filters(bench, &disk, "on disk, 8 MiB pool");
    drop(disk);
    let case = Case::new(format!("parcel windows / moves, {OPERATIONS} ops/worker"), "operations").threads(4);
    bench.measure_mixed(case, || loaded(None, (&parcels, &roads)), |fresh| sustained(fresh, 4), |fresh| checked(fresh, parcels.len()));
}

fn filters(bench: &Bench, db: &Database, label: &str) {
    for (name, sql, value) in [
        ("parcel windows", WINDOW, (|n| geometry(&shapes::window(n, WIDTH))) as fn(u64) -> Value),
        ("roads within 150", NEAR, |n| geometry(&shapes::parcel(n * 97))),
    ] {
        bench.measure_sampled(Case::new(format!("{name}, {label}"), "queries"), || {
            let session = db.session("B");
            let prepared = session.prepare(sql).unwrap();
            (0..QUERIES)
                .map(|n| {
                    let value = value(n);
                    timed(|| drained(&session, &prepared, value))
                })
                .collect()
        });
    }
}

/// Opens `prepared` with `value`, fetches every row and commits.
fn drained(session: &Session, prepared: &Prepared, value: Value) {
    let mut cursor = session.open_prepared(prepared, &[value]).unwrap();
    while let Some(row) = cursor.fetch().unwrap() {
        black_box(row);
    }
    drop(cursor);
    session.commit().unwrap();
}

fn timed(work: impl FnOnce()) -> Duration {
    let now = Instant::now();
    work();
    now.elapsed()
}

fn geometry(wkt: &str) -> Value {
    Geometry::from_wkt(wkt, 0).unwrap().to_value()
}

fn complete(db: &Database, sql: &str, values: &[Value]) -> usize {
    let session = db.session("B");
    let prepared = session.prepare(sql).unwrap();
    let mut cursor = session.open_prepared(&prepared, values).unwrap();
    let mut found = 0;
    while let Some(row) = cursor.fetch().unwrap() {
        black_box(row);
        found += 1;
    }
    drop(cursor);
    session.commit().unwrap();
    found
}

/// The two tables, in memory, or on disk in `bench`'s directory with a
/// small pool; indexed, their statistics gathered.
fn loaded(on_disk: Option<&Bench>, (parcels, roads): (&[String], &[String])) -> Database {
    let db = Database::new();
    if let Some(bench) = on_disk {
        db.set_setting("STORAGE.POOL_BYTES", &Value::Exact(Decimal::new(SMALL_POOL, 0))).unwrap();
        let dir = bench.path("realistic");
        let _ = std::fs::remove_dir_all(&dir);
        db.open_files(&(Arc::new(OsFileSystem) as Arc<dyn FileSystem>), &dir, &OpenOptions::default()).unwrap();
    }
    db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".into())).unwrap();
    db.set_lock_wait(LockWait::Wait).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE PARCELS (ID INTEGER PRIMARY KEY, G GEOMETRY(0)) CREATE TABLE ROADS (ID INTEGER PRIMARY KEY, G GEOMETRY(0))").unwrap();
    let session = db.session("B");
    for (table, shapes) in [("PARCELS", parcels), ("ROADS", roads)] {
        let insert = session.prepare(&format!("INSERT INTO {table} VALUES (?, ?)")).unwrap();
        for (id, wkt) in shapes.iter().enumerate() {
            session.execute_prepared(&insert, &[Value::Exact(Decimal::new(i128::try_from(id).unwrap(), 0)), geometry(wkt)]).unwrap();
        }
        session.commit().unwrap();
        db.create_index(&format!("{table}_G"), &format!("B.{table}"), &["G"], "rtree").unwrap();
        db.analyze(&format!("B.{table}")).unwrap();
    }
    db
}

fn plans(db: &Database) {
    let session = db.session("B");
    for (sql, wkt) in [(WINDOW, shapes::window(0, WIDTH)), (NEAR, shapes::parcel(0))] {
        let filled = sql.replace('?', &format!("ST_GEOMFROMTEXT('{wkt}')"));
        super::harness::note(&session.physical_plan(&filled, &Params::new()).unwrap());
    }
    super::harness::note(&session.physical_plan(JOIN, &Params::new()).unwrap());
}

fn sustained(db: &Database, threads: usize) -> Samples {
    let start = Barrier::new(threads);
    let samples = Mutex::new(Samples::default());
    on_threads(threads, |thread| {
        let a = db.session("B");
        let (query, update) = (a.prepare(WINDOW).unwrap(), a.prepare("UPDATE PARCELS SET G = ? WHERE ID = ?").unwrap());
        let mut own = Samples::default();
        start.wait();
        for n in 0..OPERATIONS {
            if n % WRITE_EVERY == 0 {
                let id = ((n / WRITE_EVERY) * threads + thread) as u64;
                let values = [geometry(&shapes::moved(id, n as u64 + 1)), Value::Exact(Decimal::new(i128::from(id), 0))];
                own.writes.push(timed(|| {
                    assert_eq!(a.execute_prepared(&update, &values).unwrap(), Completion::Success);
                    a.commit().unwrap();
                }));
            } else {
                let value = geometry(&shapes::window((thread * OPERATIONS + n) as u64, WIDTH));
                own.reads.push(timed(|| drained(&a, &query, value)));
            }
        }
        samples.lock().unwrap().append(own);
    });
    samples.into_inner().unwrap()
}

fn checked(db: &Database, parcels: usize) {
    assert_eq!(complete(db, "SELECT ID FROM PARCELS", &[]), parcels);
}
