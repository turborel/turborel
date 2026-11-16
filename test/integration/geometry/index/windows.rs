//! Numeric search windows through geometry functions and R-tree probes.

use super::populated;
use crate::common::{exec, rows};
use turborel::error::Code;
use turborel::params::Params;
use turborel::value::Value;

/// Verifies: [EXTENSION] D82: numeric window construction finds the same
/// boundary points and polygon-hole exclusions with and without an index.
/// Every prepared execution supplies new coordinate parameters.
#[test]
fn numeric_geometry_windows_match_scans_and_bind_parameters() {
    let db = populated();
    let a = db.session("A");
    let fixed = "SELECT ID FROM SHAPES WHERE ST_INTERSECTS(G, ST_MAKEENVELOPE(4, 4, 6, 6, 4326)) = 1 ORDER BY ID";
    let plan = a.physical_plan(fixed, &Params::new()).unwrap();
    assert!(plan.contains("SHAPES_G"), "{plan}");
    assert_eq!(rows(&a, fixed).unwrap(), ["4", "5", "6"]);
    for indexed in [true, false] {
        if !indexed {
            a.commit().unwrap();
            db.drop_index("SHAPES_G").unwrap();
        }
        let query = a.prepare("SELECT ID FROM SHAPES WHERE ST_INTERSECTS(G, ST_MAKEENVELOPE(?, ?, ?, ?, 4326)) = 1 ORDER BY ID").unwrap();
        for n in 20_i32..36_i32 {
            let lower = f64::from(n);
            let upper = f64::from(n + 2);
            let params = [Value::Approx(lower), Value::Approx(lower), Value::Approx(upper), Value::Approx(upper)];
            let mut cursor = a.open_prepared(&query, &params).unwrap();
            for id in n..=n + 2 {
                assert_eq!(cursor.fetch().unwrap().unwrap()[0].to_string(), id.to_string());
            }
            assert!(cursor.fetch().unwrap().is_none());
        }
        assert_eq!(rows(&a, fixed).unwrap(), ["4", "5", "6"]);
        let invalid = "SELECT ID FROM SHAPES WHERE ID = 9999 AND ST_INTERSECTS(G, ST_MAKEENVELOPE(0, 0, 0, 1, 4326)) = 1";
        assert_eq!(rows(&a, invalid).unwrap_err(), Code::Data);
    }
}

/// Verifies: [EXTENSION] D82: SQL extrema distinguish NULL/EMPTY and
/// non-point values. An invalid rectangle rolls back the whole insertion;
/// empty inputs and skipped CASE branches preserve D15 lazy errors.
#[test]
fn window_functions_preserve_sql_errors_and_atomic_writes() {
    let db = super::super::registered();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (ID INTEGER, G GEOMETRY(0))").unwrap();
    let a = db.session("A");
    let invalid = "INSERT INTO T VALUES (1, ST_MAKEENVELOPE(0, 0, 1, 1)), (2, ST_MAKEENVELOPE(1, 0, 0, 1))";
    assert_eq!(exec(&a, invalid).unwrap_err(), Code::Data);
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM T").unwrap(), ["0"]);
    assert_eq!(rows(&a, "SELECT ST_MAKEENVELOPE(0, 0, 0, 1) FROM T").unwrap(), Vec::<String>::new());
    a.run("INSERT INTO T VALUES (1, ST_MAKEENVELOPE(-3, -2, 7, 8)), (2, ST_GEOMFROMTEXT('POLYGON EMPTY')), (3, NULL)").unwrap();
    assert_eq!(
        rows(&a, "SELECT ID, ST_MINX(G), ST_MAXX(G), ST_MINY(G), ST_MAXY(G) FROM T ORDER BY ID").unwrap(),
        ["1,-3E0,7E0,-2E0,8E0", "2,NULL,NULL,NULL,NULL", "3,NULL,NULL,NULL,NULL"]
    );
    let query = "SELECT CASE WHEN ID > 0 THEN ID ELSE ST_SRID(ST_MAKEENVELOPE(0, 0, 0, 1)) END FROM T ORDER BY ID";
    assert_eq!(rows(&a, query).unwrap(), ["1", "2", "3"]);
    assert_eq!(rows(&a, "VALUES ST_MAKEENVELOPE(0, 0, 1, 1, 1.5)").unwrap_err(), Code::SyntaxRule);
    assert_eq!(rows(&a, "VALUES ST_MAKEENVELOPE(0, 0, 1, 1, 2147483648)").unwrap_err(), Code::Data);
}
