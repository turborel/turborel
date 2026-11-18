//! Complete spatial joins with holes, concave boundaries, clustered
//! coordinates and lines. Integer geometry checks validate every pair
//! outside timing; larger builds span multiple boundary-mask leaves.

#![allow(
    clippy::unwrap_used,
    clippy::as_conversions,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::indexing_slicing,
    missing_docs,
    reason = "benchmark fixtures and independent integer oracles (RULES.md R-DEV-3)"
)]

#[path = "geometry_shapes/fixture.rs"]
mod fixture;
mod harness;
#[path = "geometry_shapes/spill.rs"]
mod spill;

use std::hint::black_box;
use std::sync::Barrier;

use fixture::{Distribution, Feature, Polygon, Shape, Workload};
use harness::{Bench, Case, on_threads};
use turborel::db::Database;
use turborel::geometry::Geometry;
use turborel::params::Params;
use turborel::txn::LockWait;
use turborel::value::{Decimal, Value};

const JOIN: &str = "SELECT F.ID, R.ID FROM REGIONS R JOIN FEATURES F ON ST_INTERSECTS(F.G, R.G) = 1";

fn main() {
    let bench = Bench::new("geometry_shapes");
    let spill = std::env::var("TURBOREL_SHAPES_SPILL").unwrap_or_else(|_| "disk".into());
    assert!(spill == "disk" || spill == "memory", "TURBOREL_SHAPES_SPILL must be disk or memory");
    let temp = (spill == "disk").then(|| bench.path("spills"));
    for workload in [
        Workload { features: 100_000, regions: 32, distribution: Distribution::Uniform, shape: Shape::Holed, lines: false },
        Workload { features: 100_000, regions: 256, distribution: Distribution::Clustered, shape: Shape::Holed, lines: false },
        Workload { features: 100_000, regions: 256, distribution: Distribution::Uniform, shape: Shape::Comb, lines: false },
        Workload { features: 20_000, regions: 256, distribution: Distribution::Uniform, shape: Shape::Holed, lines: true },
    ] {
        let features = workload.features();
        let regions = workload.regions();
        let db = loaded(&features, &regions, temp.as_deref());
        validate(&db, &features, &regions);
        for threads in [1_usize, 4, 8] {
            let case = Case::new(format!("{}; spill={spill}", workload.label()), "joins").threads(threads);
            bench.measure(case, || joined(&db, threads));
        }
        let session = db.session("B");
        harness::note(&format!("{}:\n{}", workload.label(), session.physical_plan(JOIN, &Params::new()).unwrap()));
    }
}

fn loaded(features: &[Feature], regions: &[Polygon], temp: Option<&std::path::Path>) -> Database {
    let db = Database::new();
    if let Some(temp) = temp {
        db.set_spill(Box::new(spill::OnDisk::new(temp))).unwrap();
    }
    db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".into())).unwrap();
    db.set_lock_wait(LockWait::Wait).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE FEATURES (ID INTEGER PRIMARY KEY, G GEOMETRY(0)) CREATE TABLE REGIONS (ID INTEGER PRIMARY KEY, G GEOMETRY(0))").unwrap();
    let session = db.session("B");
    for (table, geometries) in
        [("FEATURES", features.iter().map(Feature::wkt).collect::<Vec<_>>()), ("REGIONS", regions.iter().map(Polygon::wkt).collect())]
    {
        let insert = session.prepare(&format!("INSERT INTO {table} VALUES (?, ?)")).unwrap();
        for (id, geometry) in geometries.into_iter().enumerate() {
            let id = Value::Exact(Decimal::new(i128::try_from(id).unwrap(), 0));
            session.execute_prepared(&insert, &[id, Geometry::from_wkt(&geometry, 0).unwrap().to_value()]).unwrap();
        }
    }
    session.commit().unwrap();
    db.create_index("FEATURES_G", "B.FEATURES", &["G"], "rtree").unwrap();
    db
}

fn joined(db: &Database, threads: usize) -> u64 {
    let start = Barrier::new(threads);
    on_threads(threads, |_| {
        let session = db.session("B");
        start.wait();
        let mut cursor = session.open_cursor(JOIN).unwrap();
        while let Some(row) = cursor.fetch().unwrap() {
            black_box(row);
        }
        drop(cursor);
        session.commit().unwrap();
    });
    threads as u64
}

fn validate(db: &Database, features: &[Feature], regions: &[Polygon]) {
    let mut expected = Vec::new();
    for (region_id, region) in regions.iter().enumerate() {
        for (feature_id, feature) in features.iter().enumerate() {
            if region.intersects(feature) {
                expected.push((feature_id, region_id));
            }
        }
    }
    let session = db.session("B");
    let mut cursor = session.open_cursor(JOIN).unwrap();
    let mut actual = Vec::new();
    while let Some(row) = cursor.fetch().unwrap() {
        actual.push((row[0].to_string().parse::<usize>().unwrap(), row[1].to_string().parse::<usize>().unwrap()));
    }
    drop(cursor);
    session.commit().unwrap();
    actual.sort_unstable();
    expected.sort_unstable();
    assert_eq!(actual, expected, "all intersection pairs must match the integer oracle");
    harness::note(&format!("{} features × {} regions: {} validated pairs", features.len(), regions.len(), actual.len()));
}
