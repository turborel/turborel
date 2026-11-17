//! Spatial streaming needs a planner proof that every SQL expression is
//! non-raising. Merely recognizing a function name is insufficient.

use super::*;
use crate::geometry::{FUNCTIONS, GeometryType};
use crate::value::Value;

fn spatial() -> Database {
    let db = Database::new();
    db.register_type(Box::new(GeometryType)).unwrap();
    for function in FUNCTIONS {
        db.register_scalar_function(Box::new(function)).unwrap();
    }
    db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".into())).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE P (ID INTEGER, G GEOMETRY(0)) CREATE TABLE R (ID INTEGER, G GEOMETRY(0))")
        .unwrap();
    let a = db.session("A");
    // Enough rows for the default cost model to choose indexed joins.
    for id in 0_i32..2000_i32 {
        a.run(&format!("INSERT INTO P VALUES ({id}, ST_POINT({id}, {id}))")).unwrap();
    }
    a.run("INSERT INTO R VALUES (1, ST_MAKEENVELOPE(-1, -1, 99, 99)), (2, ST_MAKEENVELOPE(-1, -1, 99, 99)), (3, NULL), (4, ST_GEOMFROMTEXT('POINT EMPTY'))").unwrap();
    a.commit().unwrap();
    db.create_index("P_G", "A.P", &["G"], "rtree").unwrap();
    db
}

/// Verifies: [DESIGN] D15, D85: a proven same-SRID intersection join
/// streams instead of retaining its entire result before the first fetch.
/// Both argument orders and non-raising residuals preserve this proof;
/// every expected pair is still fetched. Without the index, the general
/// product conservatively checks its spatial calls eagerly.
#[test]
fn proven_spatial_joins_stream_without_eager_result_spill() {
    let db = spatial();
    let a = db.session("A");
    let budget = Arc::clone(&db.snapshot().unwrap().memory);
    let queries = [
        "SELECT P.ID, R.ID FROM R JOIN P ON ST_INTERSECTS(P.G, R.G) = 1",
        "SELECT P.ID, R.ID FROM R JOIN P ON 1 = ST_INTERSECTS(R.G, P.G) AND P.ID >= 0",
    ];
    for query in queries {
        let plan = a.physical_plan(query, &crate::params::Params::new()).unwrap();
        assert!(plan.contains("AdaptiveSpatialJoin"), "{plan}");
        let mut cursor = a.open_cursor(query).unwrap();
        assert!(lock_builtin(&cursor.rows).as_ref().unwrap().streaming());
        assert!(budget.reservations().iter().any(|(name, _)| name == "spatial cursor output lookahead"));
        let mut actual = Vec::new();
        while let Some(row) = cursor.fetch().unwrap() {
            actual.push((row[0].to_string(), row[1].to_string()));
        }
        let mut expected: Vec<_> =
            (0_i32..100_i32).flat_map(|id| [1_i32, 2_i32].map(move |region| (id.to_string(), region.to_string()))).collect();
        actual.sort();
        expected.sort();
        assert_eq!(actual, expected);
    }
    a.commit().unwrap();
    db.drop_index("P_G").unwrap();
    let cursor = a.open_cursor(queries[0]).unwrap();
    assert!(!lock_builtin(&cursor.rows).as_ref().unwrap().streaming());
}

/// Verifies: [DESIGN] D15, D38, D85: a same-session write settles a
/// spatial cursor through its partially fetched output group. Buffered
/// and unread pairs both retain their original values; the next query
/// sees the write. No pair is lost or repeated at the group boundary.
#[test]
fn a_write_settles_buffered_and_unread_spatial_output() {
    let db = spatial();
    let a = db.session("A");
    let query = "SELECT P.ID, R.ID FROM R JOIN P ON ST_INTERSECTS(P.G, R.G) = 1";
    let mut cursor = a.open_cursor(query).unwrap();
    let mut actual = Vec::new();
    for _ in 0_i32..2_i32 {
        let row = cursor.fetch().unwrap().unwrap();
        actual.push((row[0].to_string(), row[1].to_string()));
    }
    a.run("UPDATE P SET G = ST_POINT(10000, 10000) WHERE ID < 100").unwrap();
    while let Some(row) = cursor.fetch().unwrap() {
        actual.push((row[0].to_string(), row[1].to_string()));
    }
    let mut expected: Vec<_> =
        (0_i32..100_i32).flat_map(|id| [1_i32, 2_i32].map(move |region| (id.to_string(), region.to_string()))).collect();
    actual.sort();
    expected.sort();
    assert_eq!(actual, expected);
    a.commit().unwrap();
    assert!(a.open_cursor(query).unwrap().fetch().unwrap().is_none());
}

/// Verifies: [DESIGN] D15: an SRID error, raising ON expression or
/// raising output expression still fails at OPEN, even if an earlier row
/// could have been returned. Geometry NULLs do not hide later failures.
#[test]
fn unproved_spatial_calls_and_other_errors_still_fail_at_open() {
    let db = spatial();
    let a = db.session("A");
    for query in [
        "SELECT P.ID FROM R JOIN P ON ST_INTERSECTS(P.G, R.G) = 1 AND 1 / (P.ID - 2) > 0",
        "SELECT 1 / (P.ID - 2) FROM R JOIN P ON ST_INTERSECTS(P.G, R.G) = 1",
        "SELECT P.ID FROM R JOIN P ON ST_INTERSECTS(P.G, ST_SETSRID(R.G, 4326)) = 1",
    ] {
        let error = a.open_cursor(query).err().unwrap();
        assert_eq!(error.code, Code::Data, "{query}");
    }
}
