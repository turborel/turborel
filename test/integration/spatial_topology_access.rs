//! Topology-geometry and topology-networks through SQL [ISO/IEC
//! 13249-3:2016 (DIS, SC32 N2593) §11.3, §12.3]: isolated edges added by
//! their routine, a face's MBR kept up to date, schemas that are no
//! topology, names not known as an invocation is bound, and whole-network
//! routines refused to a user without the network's privileges.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::value::Value;

use crate::common::{failed, rows};

fn database() -> Database {
    let db = Database::new();
    db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".into())).unwrap();
    db
}

/// The SQLSTATE statement `sql` fails with in `s`.
fn state(s: &Session, sql: &str) -> String {
    s.run(sql).unwrap_err().sqlstate().as_str().to_owned()
}

/// The value of function call `call` in `s`, as text, invoked as a row
/// of an INSERT.
fn invoked(s: &Session, call: impl AsRef<str>) -> String {
    s.run(&format!("INSERT INTO R VALUES ({})", call.as_ref())).unwrap();
    let got = rows(s, "SELECT N FROM R").unwrap();
    s.run("DELETE FROM R").unwrap();
    got.concat()
}

/// The ST_Curve of the linestring through `points`.
fn curve(points: &str) -> String {
    format!("TREAT(ST_GEOMFROMTEXT('LINESTRING ({points})') AS ST_CURVE)")
}

/// A session of user A with topology `T`, a square of side 10 at the
/// origin, its face 1 inside, and table R.
fn square(db: &Database) -> Session {
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE R (N INTEGER)").unwrap();
    let a = db.session("A");
    a.run("CALL ST_INITTOPOGEO('T')").unwrap();
    for (x, y) in [(0, 0), (10, 0), (10, 10), (0, 10)] {
        invoked(&a, format!("ST_ADDISONODE('T', CAST(NULL AS INTEGER), ST_POINT({x}, {y}))"));
    }
    for (s, e, line) in [(1, 2, "0 0, 10 0"), (2, 3, "10 0, 10 10"), (3, 4, "10 10, 0 10"), (4, 1, "0 10, 0 0")] {
        invoked(&a, format!("ST_ADDEDGEMODFACE('T', {s}, {e}, {})", curve(line)));
    }
    a
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.4, §11.3.6]
/// ST_AddIsoEdge joins two isolated nodes within a face; an edge's curve
/// changed to bulge out of its face grows the face's MBR.
#[test]
fn isolated_edges_and_growing_faces() {
    let db = database();
    let a = square(&db);
    invoked(&a, "ST_ADDISONODE('T', CAST(NULL AS INTEGER), ST_POINT(2, 2))");
    invoked(&a, "ST_ADDISONODE('T', CAST(NULL AS INTEGER), ST_POINT(8, 2))");
    assert_eq!(invoked(&a, format!("ST_ADDISOEDGE('T', 5, 6, {})", curve("2 2, 8 2"))), "5");
    let edge = "SELECT START_NODE, END_NODE, LEFT_FACE, RIGHT_FACE FROM \"T\".ST_EDGE WHERE EDGE_ID = 5";
    assert_eq!(rows(&a, edge).unwrap(), ["5,6,1,1"]);
    a.run(&format!("CALL ST_CHANGEEDGEGEOM('T', 1, {})", curve("0 0, 5 -5, 10 0"))).unwrap();
    let mbr = rows(&a, "SELECT ST_MINY(MBR) FROM \"T\".ST_FACE WHERE FACE_ID = 1").unwrap();
    assert_eq!(mbr, ["-5E0"]);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.18 Description
/// 2c, 2f] ST_CreateTopoGeo of a schema without the views of a topology
/// is 2FF48 (non-existent view), and of a topology the user has no
/// privilege on 2FF63.
#[test]
fn a_schema_that_is_no_topology() {
    let db = database();
    let a = square(&db);
    a.commit().unwrap();
    db.schema("CREATE SCHEMA \"U\" AUTHORIZATION A CREATE TABLE X (N INTEGER)").unwrap();
    let g = "TREAT(ST_GEOMFROMTEXT('MULTIPOINT ((1 1))') AS ST_GEOMCOLLECTION)";
    assert_eq!(state(&a, &format!("CALL ST_CREATETOPOGEO('U', {g})")), "2FF48");
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE R (N INTEGER)").unwrap();
    assert_eq!(state(&db.session("B"), &format!("CALL ST_CREATETOPOGEO('T', {g})")), "2FF63");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.19, §12.3.18]
/// a table function's topology or network named by a column, so not
/// known as the invocation is bound, is a syntax error.
#[test]
fn names_known_as_the_invocation_is_bound() {
    let db = database();
    let a = square(&db);
    a.run("CALL ST_INITTOPONET('N')").unwrap();
    a.run("INSERT INTO R VALUES (1)").unwrap();
    let topology = "SELECT E.ERROR FROM R, TABLE(ST_VALIDATETOPOGEO(CAST(R.N AS VARCHAR(8)))) AS E";
    assert_eq!(failed(&a, topology).code, Code::SyntaxRule);
    let network = "SELECT V.ERROR FROM R, TABLE(ST_VALIDSPATIALNET(CAST(R.N AS VARCHAR(8)))) AS V";
    assert_eq!(failed(&a, network).code, Code::SyntaxRule);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3 Description 2c,
/// §12.3.15, §12.3.16] making a network of a topology or of a collection
/// is refused with 2FF65 to a user granted nothing on the network.
#[test]
fn whole_network_routines_need_privileges() {
    let db = database();
    let a = square(&db);
    a.run("CALL ST_INITTOPONET('N')").unwrap();
    a.commit().unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE R (N INTEGER)").unwrap();
    let b = db.session("B");
    assert_eq!(state(&b, "CALL ST_SPATNETFROMTGEO('N', 'T')"), "2FF65");
    let g = "TREAT(ST_GEOMFROMTEXT('MULTIPOINT ((1 1))') AS ST_GEOMCOLLECTION)";
    assert_eq!(state(&b, &format!("CALL ST_SPATNETFROMGEOM('N', {g})")), "2FF65");
}
