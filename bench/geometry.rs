//! Geometry codecs, exact relationships and SQL execution, measured
//! separately so representation costs are visible. Run through
//! `cargo xtask bench geometry --save before` (RULES.md R-BENCH-1).

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
#[path = "geometry/planar.rs"]
mod planar;
#[path = "geometry/srs.rs"]
mod srs;

#[path = "geometry/curves.rs"]
mod curves;

use std::hint::black_box;

use harness::{Bench, Case, on_threads};
use turborel::db::Database;
use turborel::functions::ScalarFunction;
use turborel::geometry::{Geometry, GeometryFunction};
use turborel::memory::MemoryBudget;
use turborel::value::Value;

/// Enough points to amortize the harness while keeping SQL scans practical.
const ROWS: u32 = 10_000;
/// The coordinate domain and query square's extent.
const SIDE: u32 = 100;
const UPPER: u32 = 75;
const LOWER: u32 = 25;
/// A complex boundary to expose repeated geometry decoding.
const EDGES: u32 = 512;
const THREADS: [usize; 3] = [1, 4, 8];
/// Include executions after the prepared statement has learned its plan.
const QUERIES: u64 = 16;
/// A scattered sample bounds the cost of the exact distance baseline.
const DISTANCE_PAIRS: usize = 1024;
const DISTANCE_STRIDE: usize = 79;
/// Longer kernel batches reduce timer and scheduling noise in comparisons.
const RELATION_ROUNDS: u64 = 32;
const DISTANCE_ROUNDS: u64 = 16;
const CODEC_ROUNDS: u32 = 4;

fn main() {
    let bench = Bench::new("geometry");
    srs::run(&bench);
    let points: Vec<_> = (0..ROWS).map(|n| Geometry::point(f64::from(n % SIDE), f64::from(n / SIDE), 0).unwrap()).collect();
    let square =
        Geometry::from_wkt(&format!("POLYGON (({LOWER} {LOWER}, {UPPER} {LOWER}, {UPPER} {UPPER}, {LOWER} {UPPER}, {LOWER} {LOWER}))"), 0)
            .unwrap();
    let complex = circle(EDGES);
    for (name, polygon) in [("square", square), ("512-edge polygon", complex)] {
        bench.measure(Case::new(format!("point/{name}, decoded"), "pairs"), || {
            for _ in 0..RELATION_ROUNDS {
                for point in &points {
                    black_box(point.intersects(black_box(&polygon)).unwrap());
                }
            }
            u64::from(ROWS) * RELATION_ROUNDS
        });
        bench.measure(Case::new(format!("point/{name}, distance"), "pairs"), || {
            for _ in 0..DISTANCE_ROUNDS {
                for n in 0..DISTANCE_PAIRS {
                    black_box(points[n * DISTANCE_STRIDE % points.len()].distance(black_box(&polygon)).unwrap());
                }
            }
            DISTANCE_PAIRS as u64 * DISTANCE_ROUNDS
        });
        codecs(&bench, name, &polygon);
        prepared(&bench, name, &points, &polygon);
        sql(&bench, name, &polygon);
    }
    for edges in [4_u32, 8, 16, 32, 64, 128] {
        prepared(&bench, &format!("{edges}-edge polygon"), &points, &circle(edges));
    }
    for teeth in [64_u32, 1_024, 8_192] {
        prepared(&bench, &format!("{}-edge comb", teeth * 4 + 4), &points, &comb(teeth));
    }
    planar::run(&bench);
    curves::run(&bench);
}

/// A comb over the point grid: a base and `teeth` narrow teeth, so a
/// horizontal ray through the teeth crosses many edges.
fn comb(teeth: u32) -> Geometry {
    let width = f64::from(SIDE) / f64::from(teeth);
    let mut coordinates = vec!["0 0".to_owned()];
    for k in 0..teeth {
        let left = f64::from(k) * width;
        let right = left + width / 2.0_f64;
        coordinates.extend([format!("{left} 10"), format!("{left} 90"), format!("{right} 90"), format!("{right} 10")]);
    }
    coordinates.extend([format!("{SIDE} 10"), format!("{SIDE} 0"), "0 0".to_owned()]);
    Geometry::from_wkt(&format!("POLYGON (({}))", coordinates.join(", ")), 0).unwrap()
}

fn circle(edges: u32) -> Geometry {
    let coordinates: Vec<_> = (0..edges)
        .map(|n| {
            let angle = f64::from(n) / f64::from(edges) * std::f64::consts::TAU;
            format!("{} {}", 50.0_f64 + 25.0_f64 * angle.cos(), 50.0_f64 + 25.0_f64 * angle.sin())
        })
        .collect();
    let text = format!("POLYGON (({}, {}))", coordinates.join(", "), coordinates[0]);
    Geometry::from_wkt(&text, 0).unwrap()
}

fn codecs(bench: &Bench, name: &str, geometry: &Geometry) {
    let wkb = geometry.to_wkb();
    let value = geometry.to_value();
    bench.measure(Case::new(format!("{name}, WKB decode"), "values").bytes(wkb.len()), || {
        for _ in 0..ROWS * CODEC_ROUNDS {
            black_box(Geometry::from_wkb(black_box(&wkb), 0).unwrap());
        }
        u64::from(ROWS * CODEC_ROUNDS)
    });
    bench.measure(Case::new(format!("{name}, SQL value decode"), "values").bytes(wkb.len()), || {
        for _ in 0..ROWS * CODEC_ROUNDS {
            black_box(Geometry::from_value(black_box(&value)).unwrap());
        }
        u64::from(ROWS * CODEC_ROUNDS)
    });
}

fn prepared(bench: &Bench, name: &str, points: &[Geometry], geometry: &Geometry) {
    let budget = MemoryBudget::new(usize::MAX);
    let value = geometry.to_value();
    let points: Vec<_> = points.iter().map(Geometry::to_value).collect();
    for (operation, function, rounds, count, stride) in [
        ("intersection", GeometryFunction::Intersects, RELATION_ROUNDS, points.len(), 1_usize),
        ("distance", GeometryFunction::Distance, DISTANCE_ROUNDS, DISTANCE_PAIRS, DISTANCE_STRIDE),
    ] {
        let call = function.specialize(&[None, Some(&value)], &budget).unwrap();
        harness::note(&format!("{name}, prepared {operation}: {} retained bytes", budget.reserved()));
        bench.measure(Case::new(format!("point/{name}, prepared {operation}"), "pairs"), || {
            for _ in 0..rounds {
                for n in 0..count {
                    black_box(call.0.call(&[points[n * stride % points.len()].clone(), value.clone()]).unwrap());
                }
            }
            count as u64 * rounds
        });
    }
    bench.measure(Case::new(format!("{name}, prepare constant"), "operands"), || {
        for _ in 0..ROWS {
            black_box(GeometryFunction::Intersects.specialize(&[None, Some(&value)], &budget).unwrap());
        }
        u64::from(ROWS)
    });
    assert_eq!(budget.reserved(), 0);
}

fn sql(bench: &Bench, name: &str, polygon: &Geometry) {
    let db = Database::new();
    db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".into())).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE POINTS (ID INTEGER, G GEOMETRY)").unwrap();
    let load = db.session("B");
    let insert = load.prepare("INSERT INTO POINTS VALUES (?, ?)").unwrap();
    for n in 0..ROWS {
        let point = Geometry::point(f64::from(n % SIDE), f64::from(n / SIDE), 0).unwrap();
        load.execute_prepared(&insert, &[Value::Exact(turborel::value::Decimal::new(i128::from(n), 0)), point.to_value()]).unwrap();
    }
    load.commit().unwrap();
    let value = polygon.to_value();
    for threads in THREADS {
        bench.measure(Case::new(format!("point/{name}, SQL scan"), "queries").threads(threads), || {
            on_threads(threads, |_| scans(&db, &value));
            QUERIES * threads as u64
        });
    }
}

fn scans(db: &Database, value: &Value) {
    let session = db.session("B");
    let select = session.prepare("SELECT ID FROM POINTS WHERE ST_INTERSECTS(G, ?) = 1").unwrap();
    for _ in 0..QUERIES {
        let mut cursor = session.open_prepared(&select, std::slice::from_ref(value)).unwrap();
        while let Some(row) = cursor.fetch().unwrap() {
            black_box(row);
        }
        drop(cursor);
        session.rollback().unwrap();
    }
}
