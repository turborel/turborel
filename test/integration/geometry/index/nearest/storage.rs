//! Nearest queries through storage engines, temporary instances and recovery.

use crate::common::{Scratch, after_crash, rows};
use crate::geometry::registered;
use turborel::db::{Database, OpenOptions};
use turborel::params::Params;

fn populate(db: &Database, temporary: bool) {
    let table = if temporary {
        "GLOBAL TEMPORARY TABLE T (ID INTEGER, G GEOMETRY(0)) ON COMMIT PRESERVE ROWS"
    } else {
        "TABLE T (ID INTEGER, G GEOMETRY(0))"
    };
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION A CREATE {table}")).unwrap();
    let a = db.session("A");
    if !temporary {
        points(&a);
        a.commit().unwrap();
    }
    db.create_index("T_G", "A.T", &["G"], "rtree").unwrap();
}

fn points(a: &turborel::db::Session) {
    for start in (0_i32..1200_i32).step_by(100) {
        let values: Vec<_> = (start..start.saturating_add(100)).map(|n| format!("({n}, ST_POINT({n}, {n}))")).collect();
        a.run(&format!("INSERT INTO T VALUES {}", values.join(","))).unwrap();
    }
}

const QUERY: &str = "SELECT ID, ST_DISTANCE(G, ST_POINT(5, 5)) AS D FROM T ORDER BY D, ID FETCH FIRST 3 ROWS ONLY";

/// Verifies: [DESIGN] D25, D29, D82: both engines refine all seven
/// geometry kinds through SQL. Same-envelope changes and rollback keep
/// exact ranking; durable reopen rebuilds and reads the same candidates.
#[test]
fn nearest_geometry_survives_rollback_and_recovery_on_both_engines() {
    for engine in ["rowstore", "columnstore"] {
        let scratch = Scratch::new(&format!("nearest-{engine}"));
        let dir = scratch.path("db");
        let open = || {
            let db = registered();
            db.use_engine(engine).unwrap();
            db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
            db
        };
        let expected = {
            let db = open();
            populate(&db, false);
            let a = db.session("A");
            for (at, text) in [
                "LINESTRING (4 5, 6 5)",
                "POLYGON ((4 4, 6 4, 6 6, 4 6, 4 4))",
                "MULTIPOINT ((5 5), EMPTY)",
                "MULTILINESTRING ((4 5, 6 5))",
                "MULTIPOLYGON (((4 4, 6 4, 6 6, 4 6, 4 4)))",
                "GEOMETRYCOLLECTION (POINT (5 5), LINESTRING EMPTY)",
            ]
            .iter()
            .enumerate()
            {
                a.run(&format!("INSERT INTO T VALUES ({}, ST_GEOMFROMTEXT('{text}'))", at.saturating_add(2000))).unwrap();
            }
            a.commit().unwrap();
            let query = QUERY.replace("3 ROWS", "10 ROWS");
            assert!(a.physical_plan(&query, &Params::new()).unwrap().contains("NearestScan"));
            let expected = rows(&a, &query).unwrap();
            a.run("UPDATE T SET G = ST_POINT(10000, 10000) WHERE ID = 5").unwrap();
            assert_ne!(rows(&a, &query).unwrap(), expected);
            a.rollback().unwrap();
            assert_eq!(rows(&a, &query).unwrap(), expected);
            a.commit().unwrap();
            db.checkpoint().unwrap();
            expected
        };
        let db = open();
        assert_eq!(rows(&db.session("A"), &QUERY.replace("3 ROWS", "10 ROWS")).unwrap(), expected);
        db.drop_index("T_G").unwrap();
        assert_eq!(rows(&db.session("A"), &QUERY.replace("3 ROWS", "10 ROWS")).unwrap(), expected);
    }
}

/// Verifies: [DESIGN] D49, D82: a temporary index can decline before
/// its pages exist, then serves only its own session's nearest geometries.
#[test]
fn nearest_geometry_temporary_indexes_are_session_local() {
    let db = registered();
    populate(&db, true);
    let (a, b) = (db.session("A"), db.session("A"));
    assert_eq!(rows(&a, QUERY).unwrap(), Vec::<String>::new());
    a.commit().unwrap();
    points(&a);
    points(&b);
    a.run("UPDATE T SET G = ST_POINT(5, 5) WHERE ID = 0").unwrap();
    b.run("UPDATE T SET G = ST_POINT(5, 5) WHERE ID = 1").unwrap();
    a.commit().unwrap();
    b.commit().unwrap();
    for (session, id) in [(&a, "0,0E0"), (&b, "1,0E0")] {
        assert!(session.physical_plan(QUERY, &Params::new()).unwrap().contains("NearestScan"));
        assert_eq!(rows(session, QUERY).unwrap()[0], id);
    }
}
