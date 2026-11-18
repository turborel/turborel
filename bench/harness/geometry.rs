//! Shared point data and integer-coordinate oracles for geometry SQL
//! benchmarks. Validation happens outside every measured operation.

use super::scatter;
use turborel::db::Database;
use turborel::geometry::Geometry;
use turborel::txn::LockWait;
use turborel::value::{Decimal, Value};

/// Point count shared by geometry filter and join measurements.
pub const ROWS: u64 = 100_000;
const SIDE: u64 = 1_000;
/// The filter workload, with each window supplied as a parameter.
pub const SELECT: &str = "SELECT ID FROM POINTS WHERE ST_INTERSECTS(G, ?) = 1";

/// A fresh registered geometry database with points and their R-tree.
pub fn database() -> Database {
    let db = Database::new();
    db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".into())).unwrap();
    db.set_lock_wait(LockWait::Wait).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE POINTS (ID INTEGER PRIMARY KEY, G GEOMETRY(0))").unwrap();
    let load = db.session("B");
    let insert = load.prepare("INSERT INTO POINTS VALUES (?, ?)").unwrap();
    for id in 0..ROWS {
        let (x, y) = position(id);
        let point = Geometry::point(f64::from(u32::try_from(x).unwrap()), f64::from(u32::try_from(y).unwrap()), 0).unwrap().to_value();
        load.execute_prepared(&insert, &[Value::Exact(Decimal::new(i128::from(id), 0)), point]).unwrap();
    }
    load.commit().unwrap();
    db.create_index("POINTS_G", "B.POINTS", &["G"], "rtree").unwrap();
    db
}

/// A square at a deterministically chosen position.
pub fn window(n: u64, width: u64) -> Value {
    let (x, y) = position(n);
    Geometry::from_wkt(&format!("POLYGON (({x} {y}, {} {y}, {} {}, {x} {}, {x} {y}))", x + width, x + width, y + width, y + width), 0)
        .unwrap()
        .to_value()
}

/// Integer coordinates scattered through the point domain.
pub fn position(n: u64) -> (u64, u64) {
    (scatter(n, ROWS) % SIDE, scatter(n * 37 + 17, ROWS) / (ROWS / SIDE))
}

/// Expected window matches, using integer comparisons alone.
pub fn ids(n: u64, width: u64) -> Vec<u64> {
    let (lx, ly) = position(n);
    (0..ROWS)
        .filter(|&id| {
            let (x, y) = position(id);
            x >= lx && x <= lx + width && y >= ly && y <= ly + width
        })
        .collect()
}

/// The nonnegative integer ID returned by a measured query.
pub fn id(value: &Value) -> u64 {
    value.to_string().parse().unwrap()
}

/// Check every window's complete result against integer inclusion.
pub fn validate_filters(db: &Database, count: u64) {
    let session = db.session("B");
    let query = session.prepare(SELECT).unwrap();
    for width in [10_u64, 500] {
        for n in 0..count {
            let mut cursor = session.open_prepared(&query, &[window(n, width)]).unwrap();
            let mut actual = Vec::new();
            while let Some(row) = cursor.fetch().unwrap() {
                actual.push(id(&row[0]));
            }
            actual.sort_unstable();
            assert_eq!(actual, ids(n, width), "window {n}, width {width}");
        }
    }
    session.commit().unwrap();
}
