//! The spatial reference systems a database defines, read by the
//! routines that look one up [ISO/IEC 13249-3:2016 (DIS, SC32 N2593)
//! §5.1.6, §5.1.30, §5.1.41, §7.1.2, §8.1.2, §20.3]: the unit forms of
//! the measures and of ST_Buffer, and ST_Transform, in the statement's
//! catalog, before a commit and after a reopening; and the columns
//! registered with a system, which hold only its values [§4.11, §20.2].

use turborel::db::{Database, OpenOptions, Session};
use turborel::error::Code;
use turborel::value::Value;

use crate::common::{Scratch, after_crash, rows, state};

fn database() -> Database {
    let db = Database::new();
    db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".into())).unwrap();
    db
}

/// World Mercator in international feet, SRID 900002.
const DEFINE: &str = "CALL ST_DEFINESRS('MERCATOR FEET', 900002, 'ACME', 2, 'PROJCS[\"Mercator feet\",GEOGCS[\"WGS 84\",DATUM[\"WGS_1984\",SPHEROID[\"WGS 84\",6378137,298.257223563]],PRIMEM[\"Greenwich\",0],UNIT[\"degree\",0.0174532925199433]],PROJECTION[\"Mercator_1SP\"],UNIT[\"foot\",0.3048]]', 'feet')";

fn one(s: &Session, sql: &str) -> String {
    let mut found = rows(s, &format!("VALUES {sql}")).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    assert_eq!(found.len(), 1, "{sql}");
    found.remove(0)
}

fn number(s: &Session, sql: &str) -> f64 {
    one(s, sql).parse().unwrap_or_else(|e| panic!("{sql}: {e}"))
}

/// The measures, a buffer and transforms in the system of SRID 900002,
/// with what each gives there.
fn measured_in_feet(s: &Session) {
    let line = "ST_GEOMFROMTEXT('LINESTRING (0 0, 1000 0)', 900002)";
    assert_eq!(one(s, &format!("ST_LENGTH({line}, 'METRE')")), "3.048E2");
    let square = "ST_GEOMFROMTEXT('POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0))', 900002)";
    assert!((number(s, &format!("ST_AREA({square}, 'METRE')")) - 9.290_304).abs() < 1e-9);
    assert!((number(s, &format!("ST_PERIMETER({square}, 'METRE')")) - 12.192).abs() < 1e-9);
    assert_eq!(one(s, "ST_DISTANCE(ST_POINT(0, 0, 900002), ST_POINT(3000, 4000, 900002), 'METRE')"), "1.524E3");
    let area = number(s, "ST_AREA(ST_BUFFER(ST_POINT(0, 0, 900002), 1, 'METRE'))");
    let disc = std::f64::consts::PI / (0.3048_f64 * 0.3048_f64);
    assert!(area < disc && area > disc * 0.99, "{area}");
    let east = number(s, "ST_X(ST_TRANSFORM(ST_POINT(1, 0, 4326), 900002))");
    assert!((east - 111_319.490_793 / 0.3048).abs() < 1e-3, "{east}");
    let back = number(s, "ST_X(ST_TRANSFORM(ST_TRANSFORM(ST_POINT(1, 0, 4326), 900002), 4326))");
    assert!((back - 1.0).abs() < 1e-9, "{back}");
    assert_eq!(one(s, "ST_SRID(ST_TRANSFORM(ST_POINT(1, 0, 4326), 900002))"), "900002");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.6, §5.1.30,
/// §5.1.41, §7.1.2, §8.1.2, §8.1.4, §20.3] a system ST_DEFINESRS defines
/// is read by the unit forms, ST_Buffer's and ST_Transform as soon as
/// its statement's catalog holds it, in its transaction; unknown before
/// (2FF59 of a unit form, 2FF15 of a transform), and after a ROLLBACK.
#[test]
fn routines_read_systems_defined() {
    let db = database();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = db.session("A");
    assert_eq!(state(&a, "VALUES ST_LENGTH(ST_GEOMFROMTEXT('LINESTRING (0 0, 1 0)', 900002), 'METRE')"), "2FF59");
    assert_eq!(state(&a, "VALUES ST_TRANSFORM(ST_POINT(1, 0, 4326), 900002)"), "2FF15");
    a.run(DEFINE).unwrap();
    measured_in_feet(&a);
    a.rollback().unwrap();
    assert_eq!(state(&a, "VALUES ST_BUFFER(ST_POINT(0, 0, 900002), 1, 'METRE')"), "2FF59");
    a.run(DEFINE).unwrap();
    a.commit().unwrap();
    measured_in_feet(&db.session("A"));
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.6] ST_Transform
/// returns a value of its own system as it is, an empty one in the other
/// system, a value of each kind of points moved, a function or a method;
/// the unspecified plane and a system it does not know cannot be reached
/// (2FF15); a null argument gives the null value.
#[test]
fn transforms_follow_their_cases() {
    let db = database();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = db.session("A");
    assert_eq!(one(&a, "ST_ASTEXT(ST_TRANSFORM(ST_POINT(7, 8, 4326), 4326))"), "POINT (7 8)");
    assert_eq!(one(&a, "ST_SRID(ST_TRANSFORM(ST_GEOMFROMTEXT('POINT EMPTY', 4326), 3857))"), "3857");
    assert_eq!(one(&a, "ST_TRANSFORM(CAST(NULL AS ST_GEOMETRY), 3857)"), "NULL");
    let polygon = "ST_GEOMFROMTEXT('POLYGON ((0 0, 1 0, 1 1, 0 0))', 4326)";
    assert_eq!(one(&a, &format!("ST_GEOMETRYTYPE(({polygon}).ST_TRANSFORM(3857))")), "ST_Polygon");
    let x = number(&a, &format!("ST_MAXX(ST_TRANSFORM({polygon}, 3857))"));
    assert!((x - 111_319.490_793).abs() < 1e-3, "{x}");
    assert_eq!(state(&a, "VALUES ST_TRANSFORM(ST_POINT(1, 1), 4326)"), "2FF15");
    assert_eq!(state(&a, "VALUES ST_TRANSFORM(ST_POINT(1, 1, 4326), 77)"), "2FF15");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §20.3] the systems
/// defined are read again after a crash and a reopening, by the unit
/// forms and ST_Transform.
#[test]
fn systems_defined_are_read_after_reopening() {
    let scratch = Scratch::new("spatial-systems-reopen");
    let dir = scratch.path("db");
    let open = || {
        let db = database();
        db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = db.session("A");
    a.run(DEFINE).unwrap();
    a.commit().unwrap();
    drop((a, db));
    let db = open();
    measured_in_feet(&db.session("A"));
    db.checkpoint().unwrap();
    db.close().unwrap();
    drop(db);
    measured_in_feet(&open().session("A"));
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.11, §20.2] a
/// column registered with a system holds only its values: an INSERT or
/// UPDATE of another SRID is refused (SQLSTATE 23000), and so is the
/// registration of a column that holds one; a column of a type naming an
/// SRID no system has holds its values, as no rule asks it defined.
#[test]
fn registered_columns_hold_their_systems_values() {
    let db = database();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (ID INTEGER, G ST_GEOMETRY, H ST_GEOMETRY, K ST_POINT(123456))").unwrap();
    let a = db.session("A");
    a.run(DEFINE).unwrap();
    a.run("CALL ST_REGISTERGEOMETRYCOLUMN('A', 'T', 'G', 'MERCATOR FEET')").unwrap();
    a.run("INSERT INTO T VALUES (1, ST_POINT(1, 1, 900002), ST_POINT(1, 1), ST_POINT(1, 1, 123456))").unwrap();
    a.run("INSERT INTO T VALUES (2, NULL, ST_POINT(1, 1, 4326), NULL)").unwrap();
    a.commit().unwrap();
    let code = |sql: &str| a.run(sql).unwrap_err().code;
    assert_eq!(code("INSERT INTO T VALUES (3, ST_POINT(1, 1), NULL, NULL)"), Code::Constraint);
    assert_eq!(code("UPDATE T SET G = ST_POINT(1, 1, 4326) WHERE ID = 1"), Code::Constraint);
    assert_eq!(code("CALL ST_REGISTERGEOMETRYCOLUMN('A', 'T', 'H', 'WGS 84')"), Code::Constraint);
    a.rollback().unwrap();
    a.run("UPDATE T SET G = ST_TRANSFORM(ST_POINT(1, 1, 4326), 900002) WHERE ID = 2").unwrap();
    a.run("DELETE FROM T WHERE ID = 1").unwrap();
    a.run("CALL ST_REGISTERGEOMETRYCOLUMN('A', 'T', 'H', 'WGS 84')").unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&a, "SELECT ID, ST_SRID(G), ST_SRID(H) FROM T").unwrap(), ["2,900002,4326"]);
}
