//! Topology-geometry through SQL [ISO/IEC 13249-3:2016 (DIS, SC32 N2593)
//! §11]: a topology made by ST_InitTopoGeo, edited by its procedures and
//! functions, read by its table functions and ST_GetFaceGeometry, in a
//! transaction and through recovery (RULES.md D125).

use turborel::db::{Database, OpenOptions, Session};
use turborel::value::Value;

use crate::common::{Scratch, after_crash, failed, rows};

fn database() -> Database {
    let db = Database::new();
    db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".into())).unwrap();
    db
}

/// The SQLSTATE statement `sql` fails with in `s`.
fn state(s: &Session, sql: &str) -> String {
    s.run(sql).unwrap_err().sqlstate().as_str().to_owned()
}

/// The SQLSTATE query `sql` fails with in `s`.
fn read_state(s: &Session, sql: &str) -> String {
    failed(s, sql).sqlstate().as_str().to_owned()
}

/// The value of function call `call` in `s`, as text: invoked where a
/// function that possibly modifies SQL-data may be, a row of an INSERT.
fn invoked(s: &Session, call: impl AsRef<str>) -> String {
    let call = call.as_ref();
    s.run(&format!("INSERT INTO R VALUES ({call})")).unwrap();
    let got = rows(s, "SELECT N FROM R").unwrap();
    s.run("DELETE FROM R").unwrap();
    got.concat()
}

/// A session of user A with topology `T` made and a table R to take
/// values of the topology's functions.
fn topology(db: &Database) -> Session {
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE R (N INTEGER)").unwrap();
    let a = db.session("A");
    a.run("CALL ST_INITTOPOGEO('T')").unwrap();
    a
}

/// The ST_Curve of the linestring through `points`.
fn curve(points: &str) -> String {
    format!("TREAT(ST_GEOMFROMTEXT('LINESTRING ({points})') AS ST_CURVE)")
}

/// The square of side 10 at the origin: nodes 1 to 4 counterclockwise,
/// edges 1 to 4 along its sides, face 1 inside.
fn square(a: &Session) {
    for (x, y) in [(0, 0), (10, 0), (10, 10), (0, 10)] {
        invoked(a, format!("ST_ADDISONODE('T', CAST(NULL AS INTEGER), ST_POINT({x}, {y}))"));
    }
    for (s, e, line) in [(1, 2, "0 0, 10 0"), (2, 3, "10 0, 10 10"), (3, 4, "10 10, 0 10"), (4, 1, "0 10, 0 0")] {
        invoked(a, format!("ST_ADDEDGEMODFACE('T', {s}, {e}, {})", curve(line)));
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.1, §11.3.1,
/// §11.3.5, §11.3.13, §11.3.16, §11.3.17] ST_InitTopoGeo makes a schema of
/// three views holding the universal face; nodes and edges added through
/// SQL close a ring into a face, whose edges and geometry are read back.
#[test]
fn a_ring_of_edges_makes_a_face() {
    let db = database();
    let a = topology(&db);
    assert_eq!(rows(&a, "SELECT FACE_ID, MBR FROM \"T\".ST_FACE").unwrap(), ["0,NULL"]);
    square(&a);
    assert_eq!(rows(&a, "SELECT FACE_ID FROM \"T\".ST_FACE ORDER BY FACE_ID").unwrap(), ["0", "1"]);
    let edges =
        "SELECT EDGE_ID, START_NODE, END_NODE, NEXT_LEFT_EDGE, NEXT_RIGHT_EDGE, LEFT_FACE, RIGHT_FACE FROM \"T\".ST_EDGE ORDER BY EDGE_ID";
    assert_eq!(rows(&a, edges).unwrap(), ["1,1,2,2,-4,1,0", "2,2,3,3,-1,1,0", "3,3,4,4,-2,1,0", "4,4,1,1,-3,1,0"]);
    assert_eq!(rows(&a, "SELECT SEQUENCE, EDGE FROM TABLE(ST_GETFACEEDGES('T', 1)) AS E").unwrap(), ["1,1", "2,2", "3,3", "4,4"]);
    assert_eq!(
        rows(&a, "SELECT ST_ASTEXT(ST_GETFACEGEOMETRY('T', 1)) FROM (VALUES 1) AS X").unwrap(),
        ["POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0))"]
    );
    assert_eq!(rows(&a, "SELECT * FROM TABLE(ST_VALIDATETOPOGEO('T')) AS V").unwrap(), Vec::<String>::new());
    assert_eq!(state(&a, "CALL ST_INITTOPOGEO('T')"), "2FF56");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.2, §11.3.3,
/// §11.3.6 to §11.3.15] the procedures run by CALL and the functions in a
/// row of an INSERT split, heal, move, change and remove, and a ROLLBACK
/// undoes their rows; a function is refused in a query and by CALL.
#[test]
fn edits_through_sql_and_rollback() {
    let db = database();
    let a = topology(&db);
    square(&a);
    a.commit().unwrap();
    assert_eq!(invoked(&a, "ST_ADDEDGENEWFACES('T', 1, 3, ".to_owned() + &curve("0 0, 10 10") + ")"), "5");
    assert_eq!(rows(&a, "SELECT FACE_ID FROM \"T\".ST_FACE ORDER BY FACE_ID").unwrap(), ["0", "2", "3"]);
    assert_eq!(invoked(&a, "ST_REMEDGENEWFACE('T', 5)"), "4");
    assert_eq!(invoked(&a, "ST_MODEDGESPLIT('T', 1, ST_POINT(4, 0))"), "5");
    assert_eq!(invoked(&a, "ST_NEWEDGESSPLIT('T', 2, ST_POINT(10, 5))"), "6");
    a.run("CALL ST_MODEDGEHEAL('T', 1, 5)").unwrap();
    assert_eq!(invoked(&a, "ST_NEWEDGEHEAL('T', 6, 7)"), "8");
    let n = invoked(&a, "ST_ADDISONODE('T', CAST(NULL AS INTEGER), ST_POINT(5, 5))");
    a.run(&format!("CALL ST_MOVEISONODE('T', {n}, ST_POINT(6, 6))")).unwrap();
    a.run(&format!("CALL ST_REMISONODE('T', {n})")).unwrap();
    a.run(&format!("CALL ST_CHANGEEDGEGEOM('T', 3, {})", curve("10 10, 5 9, 0 10"))).unwrap();
    a.run("CALL ST_REMEDGEMODFACE('T', 3)").unwrap();
    assert_eq!(rows(&a, "SELECT * FROM TABLE(ST_VALIDATETOPOGEO('T')) AS V").unwrap(), Vec::<String>::new());
    assert_eq!(rows(&a, "SELECT FACE_ID FROM \"T\".ST_FACE ORDER BY FACE_ID").unwrap(), ["0"]);
    a.rollback().unwrap();
    assert_eq!(rows(&a, "SELECT FACE_ID FROM \"T\".ST_FACE ORDER BY FACE_ID").unwrap(), ["0", "1"]);
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM \"T\".ST_EDGE").unwrap(), ["4"]);
    assert_eq!(failed(&a, "SELECT ST_REMEDGENEWFACE('T', 1) FROM R").code, turborel::error::Code::SyntaxRule);
    assert_eq!(a.run("CALL ST_REMEDGENEWFACE('T', 1)").unwrap_err().code, turborel::error::Code::SyntaxRule);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3, §22] the
/// exceptions the routines raise through SQL: a null argument, a name no
/// schema may have or no topology has, a privilege not granted, a
/// coincident node, an edge that does not exist, and those of
/// ST_CreateTopoGeo; a user granted SELECT on the views edits the
/// topology by its routines.
#[test]
fn exceptions_and_privileges() {
    let db = database();
    let a = topology(&db);
    square(&a);
    a.commit().unwrap();
    let add = |name: &str| format!("INSERT INTO R VALUES (ST_ADDISONODE({name}, CAST(NULL AS INTEGER), ST_POINT(5, 5)))");
    assert_eq!(state(&a, &add("CAST(NULL AS VARCHAR(1))")), "2FF03");
    assert_eq!(state(&a, &add("''")), "2FF62");
    assert_eq!(state(&a, &add("'NOWHERE'")), "2FF62");
    assert_eq!(state(&a, "INSERT INTO R VALUES (ST_ADDISONODE('T', CAST(NULL AS INTEGER), ST_POINT(0, 0)))"), "2FF28");
    assert_eq!(state(&a, "CALL ST_REMISOEDGE('T', 99)"), "2FF43");
    assert_eq!(state(&a, "CALL ST_REMISONODE('T', CAST(NULL AS INTEGER))"), "2FF03");
    assert_eq!(read_state(&a, "SELECT * FROM TABLE(ST_GETFACEEDGES('T', 9)) AS E"), "2FF44");
    assert_eq!(read_state(&a, "SELECT * FROM TABLE(ST_VALIDATETOPOGEO('NOWHERE')) AS E"), "2FF62");
    let collection = "TREAT(ST_GEOMFROMTEXT('MULTIPOINT ((1 1))') AS ST_GEOMCOLLECTION)";
    assert_eq!(state(&a, &format!("CALL ST_CREATETOPOGEO('T', {collection})")), "2FF42");
    assert_eq!(state(&a, &format!("CALL ST_CREATETOPOGEO('NOWHERE', {collection})")), "2FF47");
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE R (N INTEGER)").unwrap();
    let b = db.session("B");
    assert_eq!(state(&b, &add("'T'")), "2FF63");
    assert_eq!(read_state(&b, "SELECT * FROM TABLE(ST_VALIDATETOPOGEO('T')) AS E"), "2FF63");
    a.run("GRANT SELECT ON \"T\".ST_NODE TO B").unwrap();
    a.run("GRANT SELECT ON \"T\".ST_EDGE TO B").unwrap();
    a.run("GRANT SELECT ON \"T\".ST_FACE TO B").unwrap();
    a.commit().unwrap();
    assert_eq!(invoked(&b, "ST_ADDISONODE('T', CAST(NULL AS INTEGER), ST_POINT(5, 5))"), "5");
    assert_eq!(b.run("INSERT INTO \"T\".ST_TOPO_NODE VALUES (9, ST_POINT(1, 1), 1)").unwrap_err().code, turborel::error::Code::Privilege);
    b.commit().unwrap();
    assert_eq!(rows(&a, "SELECT NODE_ID, CONTAINING_FACE FROM \"T\".ST_NODE WHERE NODE_ID = 5").unwrap(), ["5,1"]);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.18] a
/// collection made a topology: two crossing lines noded at their crossing,
/// a polygon's ring a face with an isolated node inside.
#[test]
fn create_from_a_collection() {
    let db = database();
    let a = topology(&db);
    let g = "TREAT(ST_GEOMFROMTEXT('GEOMETRYCOLLECTION (LINESTRING (0 0, 10 10), LINESTRING (0 10, 10 0), \
             POLYGON ((20 0, 30 0, 30 10, 20 10, 20 0)), POINT (25 5))') AS ST_GEOMCOLLECTION)";
    a.run(&format!("CALL ST_CREATETOPOGEO('T', {g})")).unwrap();
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM \"T\".ST_NODE").unwrap(), ["7"]);
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM \"T\".ST_EDGE").unwrap(), ["5"]);
    assert_eq!(rows(&a, "SELECT FACE_ID FROM \"T\".ST_FACE ORDER BY FACE_ID").unwrap(), ["0", "1"]);
    assert_eq!(rows(&a, "SELECT ST_ASTEXT(GEOMETRY) FROM \"T\".ST_NODE WHERE CONTAINING_FACE = 1").unwrap(), ["POINT (25 5)"]);
    assert_eq!(rows(&a, "SELECT * FROM TABLE(ST_VALIDATETOPOGEO('T')) AS V").unwrap(), Vec::<String>::new());
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.1, §11.3] a
/// topology's schema, views and rows survive a checkpoint, a crash and a
/// reopening; an edit of a transaction that did not commit is not
/// recovered, and the topology is edited again after.
#[test]
fn a_topology_survives_recovery() {
    let scratch = Scratch::new("spatial-topology-recovery");
    let dir = scratch.path("db");
    let open = || {
        let db = database();
        db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    let a = topology(&db);
    a.commit().unwrap();
    db.checkpoint().unwrap();
    square(&a);
    a.commit().unwrap();
    invoked(&a, &("ST_ADDEDGEMODFACE('T', 1, 3, ".to_owned() + &curve("0 0, 10 10") + ")"));
    let crashed = a;
    drop(db);
    let db = open();
    let a = db.session("A");
    let faces = "SELECT FACE_ID, ST_ASTEXT(MBR) FROM \"T\".ST_FACE ORDER BY FACE_ID";
    assert_eq!(rows(&a, faces).unwrap(), ["0,NULL", "1,POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0))"]);
    assert_eq!(invoked(&a, "ST_ADDEDGENEWFACES('T', 2, 4, ".to_owned() + &curve("10 0, 0 10") + ")"), "5");
    a.commit().unwrap();
    db.close().unwrap();
    drop((a, db, crashed));
    let db = open();
    let a = db.session("A");
    assert_eq!(rows(&a, "SELECT FACE_ID FROM \"T\".ST_FACE ORDER BY FACE_ID").unwrap(), ["0", "2", "3"]);
    assert_eq!(rows(&a, "SELECT * FROM TABLE(ST_VALIDATETOPOGEO('T')) AS V").unwrap(), Vec::<String>::new());
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3 Definitional
/// Rule 1, §20.5, Annex A.1] ST_MaxTopologyName and
/// ST_MaxTopologyOrNetworkName are reported; a topology name of so many
/// characters is taken, and one of more refused as it is assigned to the
/// parameter of that length (22001).
#[test]
fn the_longest_topology_name() {
    let db = database();
    let a = topology(&db);
    let sizings = "SELECT VARIABLE_NAME, SUPPORTED_VALUE FROM ST_INFORMTN_SCHEMA.ST_SIZINGS \
                   WHERE VARIABLE_NAME LIKE 'ST_MaxTopology%' ORDER BY VARIABLE_NAME";
    assert_eq!(rows(&a, sizings).unwrap(), ["ST_MaxTopologyName,128", "ST_MaxTopologyOrNetworkName,128"]);
    a.run(&format!("CALL ST_INITTOPOGEO('{}')", "M".repeat(128))).unwrap();
    assert_eq!(state(&a, &format!("CALL ST_INITTOPOGEO('{}')", "M".repeat(129))), "22001");
}
